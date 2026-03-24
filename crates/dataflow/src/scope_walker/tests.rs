use super::walk_file;

#[test]
fn go_basic_uses() {
    let src = b"package main\nfunc foo() { x := 1; y := x + 2; _ = y }";
    let chain = walk_file(src, "go").unwrap();
    let x = find_var(&chain, "x");
    let y = find_var(&chain, "y");
    assert!(x.is_some(), "x should be found");
    assert!(y.is_some(), "y should be found");
    assert!(!x.unwrap().uses.is_empty(), "x should have uses (read by y := x + 2)");
    assert!(!y.unwrap().uses.is_empty(), "y should have uses (read by _ = y)");
}

#[test]
fn go_reassignment() {
    let src = b"package main\nfunc foo() { x := 1; x = 2; _ = x }";
    let chain = walk_file(src, "go").unwrap();
    // x should have two bindings (declaration + assignment).
    let xs: Vec<_> = chain.scopes.iter()
        .flat_map(|s| s.vars.iter())
        .filter(|v| v.name == "x")
        .collect();
    assert!(xs.len() >= 2, "x should have at least 2 bindings, got {}", xs.len());
    // Last binding (x=2) should be read.
    let last = xs.last().unwrap();
    assert!(!last.uses.is_empty(), "last x binding should be read");
}

#[test]
fn python_basic_uses() {
    let src = b"def foo():\n    x = 1\n    y = x + 2\n    return y";
    let chain = walk_file(src, "python").unwrap();
    let x = find_var(&chain, "x");
    let y = find_var(&chain, "y");
    assert!(x.is_some(), "x should be found");
    assert!(y.is_some(), "y should be found");
    assert!(!x.unwrap().uses.is_empty(), "x should have uses");
    assert!(!y.unwrap().uses.is_empty(), "y should have uses (return y)");
}

#[test]
fn go_params_tracked() {
    let src = b"package main\nfunc foo(a int) { _ = a }";
    let chain = walk_file(src, "go").unwrap();
    let a = find_var(&chain, "a");
    assert!(a.is_some(), "a should be found");
    let a = a.unwrap();
    assert!(a.is_param, "a should be a parameter");
    assert!(!a.uses.is_empty(), "a should have uses");
}

#[test]
fn go_const_value_int() {
    let src = b"package main\nfunc foo() { x := 42 }";
    let chain = walk_file(src, "go").unwrap();
    let x = find_var(&chain, "x");
    assert!(x.is_some());
    assert_eq!(
        x.unwrap().def_value,
        Some(crate::types::ConstValue::Int(42))
    );
}

#[test]
fn unsupported_language() {
    let result = walk_file(b"hello", "brainfuck");
    assert!(result.is_err());
}

/// Helper: find the first VarBinding by name across all scopes.
fn find_var<'a>(
    chain: &'a crate::types::ScopeChain,
    name: &str,
) -> Option<&'a crate::types::VarBinding> {
    chain.scopes.iter()
        .flat_map(|s| s.vars.iter())
        .find(|v| v.name == name)
}
