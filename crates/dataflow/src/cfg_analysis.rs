//! CFG-based analyses: dead stores, uninitialized vars, unreachable code.
//!
//! These complement the scope-based analysis from `analysis.rs` by using
//! def-use chains and reaching definitions for higher precision.

use std::collections::HashSet;

use crate::cfg::Cfg;
use crate::cfg_builder::build_cfg;
use crate::def_use::build_def_use_chains;
use crate::il::IlFunction;
use crate::il_builder::build_il;
use crate::reaching_defs::{reaching_definitions, Definition};
use crate::types::{Finding, FindingKind, Severity};

/// Run all CFG-based analyses on source code.
pub fn analyze_cfg(source: &[u8], lang: &str, file: &str) -> Vec<Finding> {
    let il = match build_il(source, lang) {
        Ok(il) => il,
        Err(_) => return vec![],
    };
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(dead_stores_cfg(&cfg, func, file));
        findings.extend(uninitialized_vars(&cfg, func, file));
        findings.extend(unreachable_code(&cfg, file));
    }
    findings
}

/// Dead store: a definition that is never used (no reaching use).
fn dead_stores_cfg(cfg: &Cfg, func: &IlFunction, file: &str) -> Vec<Finding> {
    let chains = match build_def_use_chains(cfg) {
        Some(c) => c,
        None => return vec![],
    };
    chains
        .iter()
        .filter(|c| c.uses.is_empty())
        .filter(|c| !c.def.name.ident.starts_with('_'))
        .filter(|c| c.def.name.ident != "err")
        .filter(|c| !func.params.iter().any(|p| p.ident == c.def.name.ident))
        .map(|c| Finding {
            kind: FindingKind::DeadStore,
            severity: Severity::Warning,
            message: format!(
                "variable `{}` is assigned but never used",
                c.def.name.ident
            ),
            file: file.to_string(),
            span: c.def.span,
            variable: c.def.name.ident.clone(),
        })
        .collect()
}

/// Detect variables used before being defined.
///
/// A use is uninitialized if no definition of that variable reaches
/// the block where it is used (checked via the reaching-defs IN set).
fn uninitialized_vars(cfg: &Cfg, func: &IlFunction, file: &str) -> Vec<Finding> {
    let (defs, result) = match reaching_definitions(cfg) {
        Some(pair) => pair,
        None => return vec![],
    };

    let param_names: HashSet<&str> = func.params.iter().map(|p| p.ident.as_str()).collect();

    let chains = match build_def_use_chains(cfg) {
        Some(c) => c,
        None => return vec![],
    };

    let mut findings = Vec::new();
    for chain in &chains {
        let var_name = &chain.def.name.ident;
        // Skip parameters — always initialized.
        if param_names.contains(var_name.as_str()) {
            continue;
        }
        // For each use, check if any def of this variable is in the IN set.
        for u in &chain.uses {
            let in_set = &result.in_sets[u.block.index()];
            let has_reaching_def =
                has_def_in_set(&defs, var_name, in_set);
            if !has_reaching_def {
                findings.push(Finding {
                    kind: FindingKind::UninitializedVar,
                    severity: Severity::Warning,
                    message: format!(
                        "variable `{}` may be used before initialization",
                        var_name
                    ),
                    file: file.to_string(),
                    span: u.span,
                    variable: var_name.clone(),
                });
            }
        }
    }
    findings
}

/// Check whether any definition of `var_name` is set in the bitset.
fn has_def_in_set(
    defs: &[Definition],
    var_name: &str,
    set: &fixedbitset::FixedBitSet,
) -> bool {
    defs.iter()
        .any(|d| d.name.ident == var_name && set[d.id])
}

/// Detect blocks not reachable from entry.
fn unreachable_code(cfg: &Cfg, file: &str) -> Vec<Finding> {
    let reachable: HashSet<_> = cfg.reverse_postorder().into_iter().collect();
    cfg.graph
        .node_indices()
        .filter(|idx| !reachable.contains(idx))
        .filter_map(|idx| {
            let block = &cfg.graph[idx];
            if block.instrs.is_empty() {
                return None;
            }
            block.span.map(|span| Finding {
                kind: FindingKind::UnreachableCode,
                severity: Severity::Warning,
                message: "unreachable code".to_string(),
                file: file.to_string(),
                span,
                variable: String::new(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go_findings(src: &[u8]) -> Vec<Finding> {
        analyze_cfg(src, "go", "test.go")
    }

    #[test]
    fn dead_store_overwritten() {
        let f = go_findings(b"package main\nfunc foo() {\n    x := 1\n    x = 2\n    fmt.Println(x)\n}");
        let dead = f.iter().filter(|f| f.kind == FindingKind::DeadStore && f.variable == "x").count();
        assert!(dead >= 1, "x:=1 should be a dead store, got: {f:?}");
    }

    #[test]
    fn no_false_positive_for_used_var() {
        let f = go_findings(b"package main\nfunc foo() {\n    x := 1\n    fmt.Println(x)\n}");
        let dead: Vec<_> = f.iter().filter(|f| f.kind == FindingKind::DeadStore && f.variable == "x").collect();
        assert!(dead.is_empty(), "x is used, no dead store: {dead:?}");
    }

    #[test]
    fn parameters_not_flagged() {
        let f = go_findings(b"package main\nfunc foo(x int) {\n    fmt.Println(x)\n}");
        let xf: Vec<_> = f.iter().filter(|f| f.variable == "x").collect();
        assert!(xf.is_empty(), "param x should not be flagged: {xf:?}");
    }

    #[test]
    fn unreachable_empty_for_normal_func() {
        let f = go_findings(b"package main\nfunc foo() {\n    x := 1\n    fmt.Println(x)\n}");
        let ur: Vec<_> = f.iter().filter(|f| f.kind == FindingKind::UnreachableCode).collect();
        assert!(ur.is_empty(), "normal func has no unreachable code: {ur:?}");
    }

    #[test]
    fn analyze_cfg_combines_all() {
        let f = go_findings(b"package main\nfunc foo() {\n    x := 1\n    y := 2\n    fmt.Println(y)\n}");
        assert!(f.iter().any(|f| f.kind == FindingKind::DeadStore && f.variable == "x"),
            "x should be dead store: {f:?}");
    }
}
