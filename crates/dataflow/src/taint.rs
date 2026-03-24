//! Taint analysis engine — tracks tainted data from sources to sinks.
//!
//! Intraprocedural only: analyses a single function's CFG using def-use chains.

use std::collections::{HashSet, VecDeque};

use crate::cfg::Cfg;
use crate::def_use::{build_def_use_chains, DefUseChain};
use crate::il::{Expr, Instr, Offset};
use crate::types::{Severity, Span};
pub use crate::taint_rules::TaintRule;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TaintSource { pub pattern: String, pub tag: String }

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TaintSink {
    pub pattern: String, pub arg_index: i32, pub cwe: String, pub description: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Sanitizer { pub pattern: String }

#[derive(Debug, Clone, serde::Serialize)]
pub struct TaintSourceInfo { pub function: String, pub span: Span }

#[derive(Debug, Clone, serde::Serialize)]
pub struct TaintSinkInfo {
    pub function: String, pub span: Span, pub arg_index: i32, pub cwe: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TaintFinding {
    pub rule_id: String, pub source: TaintSourceInfo, pub sink: TaintSinkInfo,
    pub severity: Severity, pub message: String, pub file: String,
}

pub fn analyze_taint(cfg: &Cfg, rules: &[TaintRule], file: &str) -> Vec<TaintFinding> {
    let chains = match build_def_use_chains(cfg) { Some(c) => c, None => return vec![] };
    let mut findings = Vec::new();
    for rule in rules {
        let tainted_defs = find_tainted_defs(cfg, &rule.sources);
        let tainted_vars = propagate_taint(&tainted_defs, &chains, cfg);
        let sanitized = find_sanitized_vars(cfg, &rule.sanitizers);
        for (src, snk) in find_taint_at_sinks(cfg, &rule.sinks, &tainted_vars, &sanitized) {
            let severity = match rule.severity.as_str() {
                "error" | "ERROR" => Severity::Error,
                "warning" | "WARNING" => Severity::Warning,
                _ => Severity::Info,
            };
            let msg = format!(
                "Tainted data from `{}` flows to `{}` ({})", src.function, snk.function, rule.id
            );
            findings.push(TaintFinding {
                rule_id: rule.id.clone(), source: src, sink: snk,
                severity, message: msg, file: file.to_string(),
            });
        }
    }
    findings
}

pub(crate) fn func_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lval(lval) => {
            if let Some(Offset::Field(f)) = lval.offsets.last() {
                return Some(f.clone());
            }
            lval.name().map(|n| n.ident.clone())
        }
        _ => None,
    }
}

fn matches_pattern(name: &str, pattern: &str) -> bool {
    name == pattern || name.ends_with(pattern)
}

fn find_tainted_defs(cfg: &Cfg, sources: &[TaintSource]) -> Vec<(usize, TaintSourceInfo)> {
    let mut result = Vec::new();
    let mut def_id = 0usize;
    for idx in cfg.graph.node_indices() {
        for instr in &cfg.graph[idx].instrs {
            let is_def = matches!(
                instr, Instr::Assign { .. } | Instr::Call { result: Some(_), .. }
            );
            if let Instr::Call { result: Some(_), func, span, .. } = instr
                && let Some(fname) = func_name(func)
                && sources.iter().any(|s| matches_pattern(&fname, &s.pattern))
            {
                result.push((def_id, TaintSourceInfo { function: fname, span: *span }));
            }
            if is_def { def_id += 1; }
        }
    }
    result
}

fn propagate_taint(
    tainted_defs: &[(usize, TaintSourceInfo)], chains: &[DefUseChain], cfg: &Cfg,
) -> HashSet<String> {
    let mut tainted = HashSet::new();
    let mut queue = VecDeque::new();
    for &(def_id, _) in tainted_defs {
        if let Some(chain) = chains.get(def_id)
            && tainted.insert(chain.def.name.ident.clone())
        {
            queue.push_back(chain.def.name.ident.clone());
        }
    }
    while let Some(var) = queue.pop_front() {
        for idx in cfg.graph.node_indices() {
            for instr in &cfg.graph[idx].instrs {
                if let Instr::Assign { lval, rval, .. } = instr
                    && expr_uses_var(rval, &var)
                    && let Some(n) = lval.name()
                    && tainted.insert(n.ident.clone())
                {
                    queue.push_back(n.ident.clone());
                }
            }
        }
    }
    tainted
}

fn expr_uses_var(expr: &Expr, var: &str) -> bool {
    match expr {
        Expr::Lval(lval) => lval.name().is_some_and(|n| n.ident == var),
        Expr::BinOp { left, right, .. } => expr_uses_var(left, var) || expr_uses_var(right, var),
        Expr::UnaryOp { operand, .. } => expr_uses_var(operand, var),
        Expr::Call { func, args } => {
            expr_uses_var(func, var) || args.iter().any(|a| expr_uses_var(a, var))
        }
        Expr::Const(_) | Expr::Fixme(_) => false,
    }
}

fn find_sanitized_vars(cfg: &Cfg, sanitizers: &[Sanitizer]) -> HashSet<String> {
    let mut result = HashSet::new();
    for idx in cfg.graph.node_indices() {
        for instr in &cfg.graph[idx].instrs {
            if let Instr::Call { result: Some(lval), func, .. } = instr
                && let Some(fname) = func_name(func)
                && sanitizers.iter().any(|s| matches_pattern(&fname, &s.pattern))
                && let Some(n) = lval.name()
            {
                result.insert(n.ident.clone());
            }
        }
    }
    result
}

fn find_taint_at_sinks(
    cfg: &Cfg, sinks: &[TaintSink],
    tainted: &HashSet<String>, sanitized: &HashSet<String>,
) -> Vec<(TaintSourceInfo, TaintSinkInfo)> {
    let mut results = Vec::new();
    for idx in cfg.graph.node_indices() {
        for instr in &cfg.graph[idx].instrs {
            let (func, args, span) = match instr {
                Instr::Call { func, args, span, .. } => (func, args, span),
                _ => continue,
            };
            let fname = match func_name(func) { Some(f) => f, None => continue };
            for sink in sinks {
                if !matches_pattern(&fname, &sink.pattern) { continue; }
                if is_arg_tainted(args, sink.arg_index, tainted, sanitized) {
                    results.push((
                        TaintSourceInfo { function: "source".into(), span: *span },
                        TaintSinkInfo {
                            function: fname.clone(), span: *span,
                            arg_index: sink.arg_index, cwe: sink.cwe.clone(),
                        },
                    ));
                }
            }
        }
    }
    results
}

fn is_arg_tainted(
    args: &[Expr], idx: i32, tainted: &HashSet<String>, sanitized: &HashSet<String>,
) -> bool {
    let check = |e: &Expr| -> bool {
        if let Expr::Lval(lval) = e
            && let Some(n) = lval.name()
        {
            return tainted.contains(&n.ident) && !sanitized.contains(&n.ident);
        }
        false
    };
    if idx < 0 { args.iter().any(check) }
    else { args.get(idx as usize).is_some_and(check) }
}
