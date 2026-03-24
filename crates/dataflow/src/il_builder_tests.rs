//! Integration tests for IL builder.

#[cfg(test)]
mod tests {
    use crate::il::*;
    use crate::il_builder::build_il;

    fn has_instr(body: &[Instr], pred: impl Fn(&Instr) -> bool) -> bool {
        body.iter().any(pred)
    }

    #[test]
    fn go_simple_function() {
        let src = br#"package main
func foo() {
    x := 1
    y := x + 2
    fmt.Println(y)
}"#;
        let il = build_il(src, "go").unwrap();
        assert_eq!(il.functions.len(), 1);
        let f = &il.functions[0];
        assert_eq!(f.name, "foo");

        // x := 1
        assert!(has_instr(&f.body, |i| matches!(i, Instr::Assign { lval, rval: Expr::Const(Const::Int(1)), .. } if lval.name().map(|n| n.ident.as_str()) == Some("x"))));
        // y := x + 2
        assert!(has_instr(&f.body, |i| matches!(i, Instr::Assign { lval, rval: Expr::BinOp { .. }, .. } if lval.name().map(|n| n.ident.as_str()) == Some("y"))));
        // fmt.Println(y)
        assert!(has_instr(&f.body, |i| matches!(i, Instr::Call { .. })));
    }

    #[test]
    fn go_if_statement() {
        let src = br#"package main
func foo(x int) {
    if x > 0 {
        y := 1
    }
}"#;
        let il = build_il(src, "go").unwrap();
        assert_eq!(il.functions.len(), 1);
        let f = &il.functions[0];
        assert_eq!(f.name, "foo");
        assert_eq!(f.params.len(), 1);
        assert_eq!(f.params[0].ident, "x");
        assert!(has_instr(&f.body, |i| matches!(i, Instr::Branch { .. })));
    }

    #[test]
    fn go_for_loop() {
        let src = br#"package main
func foo() {
    for i := 0; i < 10; i++ {
        x := i
    }
}"#;
        let il = build_il(src, "go").unwrap();
        let f = &il.functions[0];
        // Should have at least an Assign (i := 0) and a Branch (i < 10)
        assert!(has_instr(&f.body, |i| matches!(i, Instr::Assign { .. })));
        assert!(has_instr(&f.body, |i| matches!(i, Instr::Branch { .. })));
    }

    #[test]
    fn python_simple_function() {
        let src = br#"def foo():
    x = 1
    y = x + 2
    return y
"#;
        let il = build_il(src, "python").unwrap();
        assert_eq!(il.functions.len(), 1);
        let f = &il.functions[0];
        assert_eq!(f.name, "foo");
        assert!(has_instr(&f.body, |i| matches!(i, Instr::Return { value: Some(_), .. })));
    }

    #[test]
    fn go_unsupported_is_fixme() {
        let src = br#"package main
func foo() {
    go bar()
}"#;
        let il = build_il(src, "go").unwrap();
        let f = &il.functions[0];
        assert!(has_instr(&f.body, |i| matches!(i, Instr::Fixme { reason, .. } if reason == "go_statement")));
    }

    #[test]
    fn go_multiple_functions() {
        let src = br#"package main
func foo() { x := 1 }
func bar() { y := 2 }
"#;
        let il = build_il(src, "go").unwrap();
        assert_eq!(il.functions.len(), 2);
        let names: Vec<&str> = il.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
    }

    #[test]
    fn unsupported_language_errors() {
        let result = build_il(b"code", "brainfuck");
        assert!(result.is_err());
    }

}
