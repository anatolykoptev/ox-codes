use crate::types::{Finding, FindingKind, ScopeChain, Severity};

/// Find variables assigned a value but never read (dead stores).
pub fn dead_stores(chain: &ScopeChain, file: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for scope in &chain.scopes {
        for var in &scope.vars {
            if var.is_param || var.is_read() || is_skip_name(&var.name) {
                continue;
            }
            if var.def_value.is_some() {
                findings.push(Finding {
                    kind: FindingKind::DeadStore,
                    severity: Severity::Warning,
                    message: format!("variable `{}` is assigned a value but never read", var.name),
                    file: file.to_string(),
                    span: var.def_site,
                    variable: var.name.clone(),
                });
            }
        }
    }
    findings
}

/// Find variables declared but never used at all.
pub fn unused_vars(chain: &ScopeChain, file: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for scope in &chain.scopes {
        for var in &scope.vars {
            if var.is_param || var.is_read() || is_skip_name(&var.name) {
                continue;
            }
            findings.push(Finding {
                kind: FindingKind::UnusedVariable,
                severity: Severity::Warning,
                message: format!("variable `{}` is declared but never used", var.name),
                file: file.to_string(),
                span: var.def_site,
                variable: var.name.clone(),
            });
        }
    }
    findings
}

/// Find variables with known constant values (informational).
pub fn const_values(chain: &ScopeChain, file: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for scope in &chain.scopes {
        for var in &scope.vars {
            if let Some(ref val) = var.def_value {
                findings.push(Finding {
                    kind: FindingKind::ConstantValue,
                    severity: Severity::Info,
                    message: format!("variable `{}` has constant value: {:?}", var.name, val),
                    file: file.to_string(),
                    span: var.def_site,
                    variable: var.name.clone(),
                });
            }
        }
    }
    findings
}

/// Run all analyses and return combined, deduplicated findings.
///
/// Dead stores subsume unused variables for the same binding, so we
/// prefer the more specific `DeadStore` finding when both apply.
pub fn analyze(chain: &ScopeChain, file: &str) -> Vec<Finding> {
    let ds = dead_stores(chain, file);
    let uv = unused_vars(chain, file);
    let cv = const_values(chain, file);

    let mut findings = Vec::with_capacity(ds.len() + uv.len() + cv.len());

    // Collect dead-store variable+span pairs for dedup.
    let ds_keys: std::collections::HashSet<(String, usize)> = ds
        .iter()
        .map(|f| (f.variable.clone(), f.span.start_byte))
        .collect();

    findings.extend(ds);

    // Only add unused_var findings that aren't already covered by dead_store.
    for f in uv {
        let key = (f.variable.clone(), f.span.start_byte);
        if !ds_keys.contains(&key) {
            findings.push(f);
        }
    }

    findings.extend(cv);
    findings
}

/// Names that are conventionally allowed to be unused.
fn is_skip_name(name: &str) -> bool {
    name.starts_with('_') || name == "err"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope_walker::walk_file;

    #[test]
    fn dead_store_overwritten() {
        let src = b"package main\nfunc foo() { x := 1; x = 2; _ = x }";
        let chain = walk_file(src, "go").unwrap();
        let ds = dead_stores(&chain, "test.go");
        // x := 1 is dead (overwritten before read).
        assert!(
            ds.iter().any(|f| f.variable == "x"),
            "expected dead store for x := 1, got: {ds:?}"
        );
    }

    #[test]
    fn unused_var_detected() {
        let src = b"package main\nfunc foo() { x := 1 }";
        let chain = walk_file(src, "go").unwrap();
        let uv = unused_vars(&chain, "test.go");
        assert!(
            uv.iter().any(|f| f.variable == "x"),
            "expected unused var x, got: {uv:?}"
        );
    }

    #[test]
    fn underscore_skip() {
        let src = b"package main\nfunc foo() { _ = getValue() }";
        let chain = walk_file(src, "go").unwrap();
        let all = analyze(&chain, "test.go");
        assert!(
            !all.iter().any(|f| f.variable == "_"
                && (f.kind == FindingKind::DeadStore || f.kind == FindingKind::UnusedVariable)),
            "_ should be skipped, got: {all:?}"
        );
    }

    #[test]
    fn const_value_detected() {
        let src = b"package main\nfunc foo() { x := 42 }";
        let chain = walk_file(src, "go").unwrap();
        let cv = const_values(&chain, "test.go");
        assert!(
            cv.iter()
                .any(|f| f.variable == "x" && f.kind == FindingKind::ConstantValue),
            "expected const value for x, got: {cv:?}"
        );
    }

    #[test]
    fn analyze_deduplicates() {
        let src = b"package main\nfunc foo() { x := 1 }";
        let chain = walk_file(src, "go").unwrap();
        let all = analyze(&chain, "test.go");
        // x should appear as DeadStore (has value) but NOT also as UnusedVariable.
        let x_findings: Vec<_> = all.iter().filter(|f| f.variable == "x").collect();
        let has_dead = x_findings.iter().any(|f| f.kind == FindingKind::DeadStore);
        let has_unused = x_findings
            .iter()
            .any(|f| f.kind == FindingKind::UnusedVariable);
        assert!(has_dead, "expected DeadStore for x");
        assert!(!has_unused, "DeadStore should subsume UnusedVariable");
    }
}
