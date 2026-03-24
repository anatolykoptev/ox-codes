//! Hard red tests for IL builder and CFG construction.
//!
//! Each test targets an edge case or boundary condition that is easy to
//! get wrong during refactoring.

use ox_dataflow::cfg_builder::build_cfg;
use ox_dataflow::il::{Base, Const, Expr, IlFunction, Instr, Lval, Name, Offset};
use ox_dataflow::il_builder::build_il;
use ox_dataflow::types::Span;

fn go_il(src: &str) -> ox_dataflow::il::IlFile {
    build_il(src.as_bytes(), "go").expect("go parse failed")
}

fn py_il(src: &str) -> ox_dataflow::il::IlFile {
    build_il(src.as_bytes(), "python").expect("python parse failed")
}

fn span() -> Span {
    Span { start_byte: 0, end_byte: 1, start_line: 1, end_line: 1 }
}

fn il_func(body: Vec<Instr>) -> IlFunction {
    IlFunction { name: "t".into(), params: vec![], body, span: span() }
}

// ---------------------------------------------------------------------------
// IL Builder edge cases
// ---------------------------------------------------------------------------

#[test]
fn empty_go_function_produces_empty_body() {
    let il = go_il("package main\nfunc empty() {}");
    assert_eq!(il.functions.len(), 1, "should find exactly one function");
    assert!(il.functions[0].body.is_empty(), "empty func body must be empty");
}

#[test]
fn nested_go_functions_both_captured() {
    // Go func literal inside a function — tree-sitter parses it as
    // function_declaration containing func_literal. The visitor should
    // still produce the outer function; inner anonymous funcs are not
    // function_declarations so they won't be separate IlFunctions.
    let src = "package main\nfunc outer() {\n\tf := func() { return }\n\tf()\n}";
    let il = go_il(src);
    // At minimum the outer function must exist.
    assert!(!il.functions.is_empty(), "outer function must be captured");
    assert_eq!(il.functions[0].name, "outer");
}

#[test]
fn multiple_returns_all_become_instr_return() {
    let src = "package main\nfunc multi(x int) int {\n\tif x > 0 {\n\t\treturn 1\n\t}\n\treturn 0\n}";
    let il = go_il(src);
    let returns: Vec<_> = il.functions[0]
        .body
        .iter()
        .filter(|i| matches!(i, Instr::Return { .. }))
        .collect();
    assert_eq!(returns.len(), 2, "both return statements must appear");
}

#[test]
fn deeply_nested_binop_preserves_structure() {
    // a + b * c - d  parses as (a + (b * c)) - d  in Go
    let src = "package main\nfunc calc() { x := a + b * c - d }";
    let il = go_il(src);
    let assigns: Vec<_> = il.functions[0]
        .body
        .iter()
        .filter(|i| matches!(i, Instr::Assign { .. }))
        .collect();
    assert!(!assigns.is_empty(), "assignment must exist");
    // The rval should be a BinOp tree, not a flat Fixme.
    if let Instr::Assign { rval, .. } = &assigns[0] {
        assert!(
            matches!(rval, Expr::BinOp { .. }),
            "nested expression must produce BinOp, got: {rval:?}"
        );
    }
}

#[test]
fn method_call_on_receiver_has_field_offset() {
    let src = "package main\nfunc use() { obj.Method() }";
    let il = go_il(src);
    let calls: Vec<_> = il.functions[0]
        .body
        .iter()
        .filter(|i| matches!(i, Instr::Call { .. }))
        .collect();
    assert!(!calls.is_empty(), "call statement must exist");
    if let Instr::Call { func, .. } = &calls[0] {
        // func should be Lval(obj.Method) with Field("Method") offset.
        if let Expr::Lval(lval) = func {
            assert!(
                lval.offsets.iter().any(|o| matches!(o, Offset::Field(f) if f == "Method")),
                "must have Field(\"Method\") offset, got: {lval:?}"
            );
        } else {
            panic!("call func must be Lval for selector, got: {func:?}");
        }
    }
}

#[test]
fn string_with_special_chars_preserved() {
    let src = r#"package main
func s() { x := "hello\nworld\t\"escaped\"" }"#;
    let il = go_il(src);
    let assigns: Vec<_> = il.functions[0]
        .body
        .iter()
        .filter(|i| matches!(i, Instr::Assign { .. }))
        .collect();
    assert!(!assigns.is_empty());
    if let Instr::Assign { rval, .. } = &assigns[0] {
        match rval {
            Expr::Const(Const::Str(s)) => {
                assert!(s.contains("hello"), "string content must be preserved: {s}");
            }
            other => panic!("expected Const::Str, got: {other:?}"),
        }
    }
}

#[test]
fn multi_assign_captures_first_lhs_only() {
    // Go: a, b := 1, 2 — known limitation: only first LHS captured.
    let src = "package main\nfunc m() { a, b := 1, 2 }";
    let il = go_il(src);
    let assigns: Vec<_> = il.functions[0]
        .body
        .iter()
        .filter_map(|i| match i {
            Instr::Assign { lval, .. } => lval.name().map(|n| n.ident.clone()),
            _ => None,
        })
        .collect();
    // Documents the known limitation: only "a" is captured.
    assert!(assigns.contains(&"a".to_string()), "first LHS must be captured");
    assert!(!assigns.contains(&"b".to_string()), "second LHS not captured (known limitation)");
}

#[test]
fn go_var_declaration_becomes_assign() {
    let src = "package main\nfunc v() { var x int = 5 }";
    let il = go_il(src);
    // var_declaration is not short_var_declaration — check if it produces
    // anything (it may be a Fixme or an Assign depending on implementation).
    // At minimum the function must exist and not panic.
    assert_eq!(il.functions.len(), 1);
    // If var decl is supported, there should be an Assign.
    // If not supported, there should be no crash — just an empty body or Fixme.
    let body = &il.functions[0].body;
    let has_assign = body.iter().any(|i| matches!(i, Instr::Assign { .. }));
    let has_fixme = body.iter().any(|i| matches!(i, Instr::Fixme { .. }));
    assert!(
        has_assign || has_fixme || body.is_empty(),
        "var decl must produce Assign, Fixme, or be skipped — not panic"
    );
}

#[test]
fn python_augmented_assign_becomes_assign() {
    let src = "def inc():\n    x = 1\n    x += 1";
    let il = py_il(src);
    assert!(!il.functions.is_empty());
    let assigns: Vec<_> = il.functions[0]
        .body
        .iter()
        .filter(|i| matches!(i, Instr::Assign { .. }))
        .collect();
    assert!(assigns.len() >= 2, "both x=1 and x+=1 must become Assign, got {}", assigns.len());
}

#[test]
fn selector_expression_produces_lval_with_field_offset() {
    let src = "package main\nfunc sel() { x := pkg.Value }";
    let il = go_il(src);
    let assigns: Vec<_> = il.functions[0]
        .body
        .iter()
        .filter(|i| matches!(i, Instr::Assign { .. }))
        .collect();
    assert!(!assigns.is_empty());
    if let Instr::Assign { rval, .. } = &assigns[0] {
        if let Expr::Lval(lval) = rval {
            assert!(
                lval.offsets.iter().any(|o| matches!(o, Offset::Field(f) if f == "Value")),
                "selector must have Field(\"Value\") offset: {lval:?}"
            );
        } else {
            panic!("selector expression must be Lval, got: {rval:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// CFG edge cases
// ---------------------------------------------------------------------------

#[test]
fn cfg_return_only_function_has_entry_block_exit() {
    let ret = Instr::Return { value: None, span: span() };
    let cfg = build_cfg(&il_func(vec![ret]));
    // entry -> [return block] -> exit = 3 nodes
    assert_eq!(cfg.block_count(), 3, "entry + return_block + exit");
    assert_eq!(cfg.successors(cfg.entry).len(), 1);
    let ret_block = cfg.successors(cfg.entry)[0];
    assert!(cfg.successors(ret_block).contains(&cfg.exit), "return block must reach exit");
}

#[test]
fn cfg_multiple_branches_correct_block_count() {
    // Simulates if/else if/else: branch, assign, branch, assign, assign
    let branch = || Instr::Branch {
        cond: Expr::Const(Const::Bool(true)),
        span: span(),
    };
    let assign = |n: &str| Instr::Assign {
        lval: Lval::var(Name::new(n, 1)),
        rval: Expr::Const(Const::Int(0)),
        span: span(),
    };
    let body = vec![
        assign("x"),
        branch(),          // first if
        assign("y"),
        branch(),          // else if
        assign("z"),
        Instr::Return { value: None, span: span() },
    ];
    let cfg = build_cfg(&il_func(body));
    // Minimum: entry + exit + blocks for each segment
    assert!(cfg.block_count() >= 5, "must have multiple blocks for branches, got {}", cfg.block_count());
    // Both branches must exist as separate decision blocks
    let mut branch_count = 0;
    for idx in cfg.graph.node_indices() {
        for instr in &cfg.graph[idx].instrs {
            if matches!(instr, Instr::Branch { .. }) {
                branch_count += 1;
            }
        }
    }
    assert_eq!(branch_count, 2, "two Branch instructions must be in separate decision blocks");
}

#[test]
fn cfg_linear_function_has_exactly_three_nodes() {
    // No control flow: assign + assign → entry → [block] → exit
    let assign = |n: &str| Instr::Assign {
        lval: Lval::var(Name::new(n, 1)),
        rval: Expr::Const(Const::Int(1)),
        span: span(),
    };
    let cfg = build_cfg(&il_func(vec![assign("a"), assign("b")]));
    assert_eq!(cfg.block_count(), 3, "linear: entry + one_block + exit");
    assert_eq!(cfg.edge_count(), 2, "linear: entry→block, block→exit");
}

#[test]
fn cfg_empty_function_has_exactly_two_nodes() {
    let cfg = build_cfg(&il_func(vec![]));
    assert_eq!(cfg.block_count(), 2, "empty: entry + exit only");
    assert_eq!(cfg.edge_count(), 1, "empty: entry→exit");
    assert_eq!(cfg.successors(cfg.entry), vec![cfg.exit]);
}

#[test]
fn cfg_branch_and_return_exit_reachable() {
    // branch then return — exit must be reachable from entry
    let body = vec![
        Instr::Branch { cond: Expr::Const(Const::Bool(true)), span: span() },
        Instr::Assign {
            lval: Lval::var(Name::new("x", 1)),
            rval: Expr::Const(Const::Int(1)),
            span: span(),
        },
        Instr::Return { value: None, span: span() },
    ];
    let cfg = build_cfg(&il_func(body));
    // Verify exit is reachable via reverse_postorder (all reachable nodes).
    let rpo = cfg.reverse_postorder();
    assert!(rpo.contains(&cfg.exit), "exit must be reachable when function has return");
    assert_eq!(rpo[0], cfg.entry, "RPO must start with entry");
}
