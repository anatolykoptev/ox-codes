//! Tests for taint analysis engine.

use crate::cfg_builder::build_cfg;
use crate::il_builder::build_il;
use crate::taint::analyze_taint;
use crate::taint_rules::default_rules;

fn analyze_go(src: &[u8]) -> Vec<crate::taint::TaintFinding> {
    let il = build_il(src, "go").unwrap();
    let rules = default_rules("go");
    let mut all = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        all.extend(analyze_taint(&cfg, &rules, "test.go"));
    }
    all
}

#[test]
fn default_go_rules_exist() {
    let rules = default_rules("go");
    assert!(rules.len() >= 2);
    assert!(rules.iter().any(|r| r.id == "sql-injection"));
    assert!(rules.iter().any(|r| r.id == "command-injection"));
}

#[test]
fn default_python_rules_exist() {
    let rules = default_rules("python");
    assert!(!rules.is_empty());
    assert!(rules.iter().any(|r| r.id == "sql-injection"));
}

#[test]
fn no_findings_for_clean_code() {
    let findings = analyze_go(
        br#"package main
func foo() {
    x := 1
    y := x + 2
    _ = y
}"#,
    );
    assert!(
        findings.is_empty(),
        "clean code should have no taint findings"
    );
}

#[test]
fn no_findings_without_sources() {
    // Code with sink-like calls but no source patterns
    let findings = analyze_go(
        br#"package main
func foo() {
    query := "SELECT 1"
    db.Exec(query)
}"#,
    );
    // "SELECT 1" is a literal, not from a source — no taint
    assert!(findings.is_empty(), "no source = no taint finding");
}

#[test]
fn taint_propagation_through_assignment() {
    // Build a CFG manually with source → assign → sink
    use crate::cfg::{Cfg, EdgeKind};
    use crate::il::*;
    use crate::taint::{TaintSink, TaintSource};
    use crate::taint_rules::TaintRule;
    use crate::types::Span;
    use smallvec::SmallVec;

    let span = Span {
        start_byte: 0,
        end_byte: 10,
        start_line: 1,
        end_line: 1,
    };

    // input = FormValue("name")
    let source_call = Instr::Call {
        result: Some(Lval::var(Name::new("input", 1))),
        func: Expr::Lval(Lval {
            base: Base::Var(Name::new("r", 0)),
            offsets: SmallVec::from_elem(Offset::Field("FormValue".into()), 1),
        }),
        args: vec![Expr::Const(Const::Str("name".into()))],
        span,
    };

    // query = input
    let assign = Instr::Assign {
        lval: Lval::var(Name::new("query", 2)),
        rval: Expr::Lval(Lval::var(Name::new("input", 1))),
        span,
    };

    // db.Exec(query)
    let sink_call = Instr::Call {
        result: None,
        func: Expr::Lval(Lval {
            base: Base::Var(Name::new("db", 0)),
            offsets: SmallVec::from_elem(Offset::Field("Exec".into()), 1),
        }),
        args: vec![Expr::Lval(Lval::var(Name::new("query", 2)))],
        span,
    };

    let mut cfg = Cfg::new();
    let block = cfg.add_block(vec![source_call, assign, sink_call], Some(span));
    cfg.add_edge(cfg.entry, block, EdgeKind::Fallthrough);
    cfg.add_edge(block, cfg.exit, EdgeKind::Fallthrough);

    let rules = vec![TaintRule {
        id: "sql-injection".into(),
        sources: vec![TaintSource {
            pattern: "FormValue".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "Exec".into(),
            arg_index: 0,
            cwe: "CWE-89".into(),
            description: "SQL injection".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }];

    let findings = analyze_taint(&cfg, &rules, "test.go");
    assert_eq!(findings.len(), 1, "should detect SQL injection");
    assert_eq!(findings[0].rule_id, "sql-injection");
    assert_eq!(findings[0].sink.cwe, "CWE-89");
    assert_eq!(findings[0].sink.function, "Exec");
}

#[test]
fn sanitizer_blocks_taint() {
    use crate::cfg::{Cfg, EdgeKind};
    use crate::il::*;
    use crate::taint::{Sanitizer, TaintSink, TaintSource};
    use crate::taint_rules::TaintRule;
    use crate::types::Span;
    use smallvec::SmallVec;

    let span = Span {
        start_byte: 0,
        end_byte: 10,
        start_line: 1,
        end_line: 1,
    };

    // input = FormValue("name")
    let source_call = Instr::Call {
        result: Some(Lval::var(Name::new("input", 1))),
        func: Expr::Lval(Lval {
            base: Base::Var(Name::new("r", 0)),
            offsets: SmallVec::from_elem(Offset::Field("FormValue".into()), 1),
        }),
        args: vec![Expr::Const(Const::Str("name".into()))],
        span,
    };

    // safe = EscapeString(input)
    let sanitize = Instr::Call {
        result: Some(Lval::var(Name::new("safe", 2))),
        func: Expr::Lval(Lval {
            base: Base::Var(Name::new("db", 0)),
            offsets: SmallVec::from_elem(Offset::Field("EscapeString".into()), 1),
        }),
        args: vec![Expr::Lval(Lval::var(Name::new("input", 1)))],
        span,
    };

    // db.Exec(safe)
    let sink_call = Instr::Call {
        result: None,
        func: Expr::Lval(Lval {
            base: Base::Var(Name::new("db", 0)),
            offsets: SmallVec::from_elem(Offset::Field("Exec".into()), 1),
        }),
        args: vec![Expr::Lval(Lval::var(Name::new("safe", 2)))],
        span,
    };

    let mut cfg = Cfg::new();
    let block = cfg.add_block(vec![source_call, sanitize, sink_call], Some(span));
    cfg.add_edge(cfg.entry, block, EdgeKind::Fallthrough);
    cfg.add_edge(block, cfg.exit, EdgeKind::Fallthrough);

    let rules = vec![TaintRule {
        id: "sql-injection".into(),
        sources: vec![TaintSource {
            pattern: "FormValue".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "Exec".into(),
            arg_index: 0,
            cwe: "CWE-89".into(),
            description: "SQL injection".into(),
        }],
        sanitizers: vec![Sanitizer {
            pattern: "EscapeString".into(),
        }],
        severity: "error".into(),
    }];

    let findings = analyze_taint(&cfg, &rules, "test.go");
    assert!(
        findings.is_empty(),
        "sanitizer should block taint: {findings:?}"
    );
}

#[test]
fn unknown_language_no_rules() {
    assert!(default_rules("ruby").is_empty());
}
