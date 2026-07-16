//! Tests for taint analysis engine.

use crate::cfg_builder::build_cfg;
use crate::il_builder::{build_il, build_il_with_ext};
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

// ---------------------------------------------------------------------------
// TSX grammar selection — taint route (issue #44)
// ---------------------------------------------------------------------------

/// A `.tsx` file where a tainted value reaches a sink call.  The JSX
/// (`const ui = <div>{tainted}</div>`) between the source and sink causes
/// the non-JSX `LANGUAGE_TYPESCRIPT` grammar to produce `ERROR` nodes that
/// drop all subsequent instructions in the function body — including the
/// `sink(tainted)` call — resulting in a false negative (the taint flow is
/// silently missed).  With the TSX grammar (selected via `build_il_with_ext`
/// with `file_ext="tsx"`), the JSX is parsed correctly and the sink call is
/// preserved, so taint analysis reports the flow.
#[test]
fn taint_tsx_sink_not_dropped() {
    use crate::taint::{TaintSink, TaintSource};
    use crate::taint_rules::TaintRule;

    let src = br#"function App() {
    const tainted = getSource();
    const ui = <div>{tainted}</div>;
    sink(tainted);
}
"#;
    let rules = vec![TaintRule {
        id: "xss".into(),
        sources: vec![TaintSource {
            pattern: "getSource".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "sink".into(),
            arg_index: 0,
            cwe: "CWE-79".into(),
            description: "XSS".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }];

    // TSX grammar (via build_il_with_ext) — sink call is preserved.
    let il = build_il_with_ext(src, "typescript", "tsx").unwrap();
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(analyze_taint(&cfg, &rules, "App.tsx"));
    }
    assert!(
        !findings.is_empty(),
        "taint flow from getSource → sink must be reported with TSX grammar; \
         IL had {} functions / {} total instrs",
        il.functions.len(),
        il.functions.iter().map(|f| f.body.len()).sum::<usize>()
    );
    assert_eq!(findings[0].rule_id, "xss");
    assert_eq!(findings[0].sink.function, "sink");
}

/// #59: object destructuring shorthand `const {token} = getSource()` must
/// track taint through to `sink(token)`. Previously `visit_var_decl` skipped
/// any non-identifier declarator name (object_pattern / array_pattern) as a
/// fail-safe, making this a silent false negative — the single most common
/// taint-source idiom in Express/Next TS. Now each leaf identifier is bound to
/// its own sid sourced from the same rval (over-approximation: every extracted
/// binding inherits the full RHS taint).
#[test]
fn taint_destructuring_object_shorthand() {
    use crate::taint::{TaintSink, TaintSource};
    use crate::taint_rules::TaintRule;

    let src = br#"function App() {
    const {token} = getSource();
    sink(token);
}
"#;
    let rules = vec![TaintRule {
        id: "xss".into(),
        sources: vec![TaintSource {
            pattern: "getSource".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "sink".into(),
            arg_index: 0,
            cwe: "CWE-79".into(),
            description: "XSS".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }];

    let il = build_il(src, "typescript").unwrap();
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(analyze_taint(&cfg, &rules, "app.ts"));
    }
    assert!(
        !findings.is_empty(),
        "const {{token}} = getSource(); sink(token) must report the taint flow; \
         IL had {} functions / {} total instrs",
        il.functions.len(),
        il.functions.iter().map(|f| f.body.len()).sum::<usize>()
    );
    assert_eq!(findings[0].rule_id, "xss");
    assert_eq!(findings[0].sink.function, "sink");
}

/// #59: array destructuring `const [a] = taintedArr; sink(a)`.
#[test]
fn taint_destructuring_array_element() {
    use crate::taint::{TaintSink, TaintSource};
    use crate::taint_rules::TaintRule;

    let src = br#"function App() {
    const [a] = getSource();
    sink(a);
}
"#;
    let rules = vec![TaintRule {
        id: "xss".into(),
        sources: vec![TaintSource {
            pattern: "getSource".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "sink".into(),
            arg_index: 0,
            cwe: "CWE-79".into(),
            description: "XSS".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }];

    let il = build_il(src, "typescript").unwrap();
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(analyze_taint(&cfg, &rules, "app.ts"));
    }
    assert!(
        !findings.is_empty(),
        "const [a] = getSource(); sink(a) must report the taint flow"
    );
}

/// #59: renamed pair `const {token: t} = getSource(); sink(t)` — the value
/// side of a `pair_pattern` is the binding (`t`), not the property key.
#[test]
fn taint_destructuring_rename_pair() {
    use crate::taint::{TaintSink, TaintSource};
    use crate::taint_rules::TaintRule;

    let src = br#"function App() {
    const {token: t} = getSource();
    sink(t);
}
"#;
    let rules = vec![TaintRule {
        id: "xss".into(),
        sources: vec![TaintSource {
            pattern: "getSource".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "sink".into(),
            arg_index: 0,
            cwe: "CWE-79".into(),
            description: "XSS".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }];

    let il = build_il(src, "typescript").unwrap();
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(analyze_taint(&cfg, &rules, "app.ts"));
    }
    assert!(
        !findings.is_empty(),
        "const {{token: t}} = getSource(); sink(t) must report the taint flow \
         (the pair value `t` is the binding, not the key `token`)"
    );
}

/// #59: rest pattern `const {...rest} = getSource(); sink(rest)`.
#[test]
fn taint_destructuring_rest() {
    use crate::taint::{TaintSink, TaintSource};
    use crate::taint_rules::TaintRule;

    let src = br#"function App() {
    const {...rest} = getSource();
    sink(rest);
}
"#;
    let rules = vec![TaintRule {
        id: "xss".into(),
        sources: vec![TaintSource {
            pattern: "getSource".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "sink".into(),
            arg_index: 0,
            cwe: "CWE-79".into(),
            description: "XSS".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }];

    let il = build_il(src, "typescript").unwrap();
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(analyze_taint(&cfg, &rules, "app.ts"));
    }
    assert!(
        !findings.is_empty(),
        "const {{...rest}} = getSource(); sink(rest) must report the taint flow"
    );
}

/// #59: nested object destructuring `const {a: {b}} = getSource(); sink(b)`.
#[test]
fn taint_destructuring_nested() {
    use crate::taint::{TaintSink, TaintSource};
    use crate::taint_rules::TaintRule;

    let src = br#"function App() {
    const {a: {b}} = getSource();
    sink(b);
}
"#;
    let rules = vec![TaintRule {
        id: "xss".into(),
        sources: vec![TaintSource {
            pattern: "getSource".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "sink".into(),
            arg_index: 0,
            cwe: "CWE-79".into(),
            description: "XSS".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }];

    let il = build_il(src, "typescript").unwrap();
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(analyze_taint(&cfg, &rules, "app.ts"));
    }
    assert!(
        !findings.is_empty(),
        "const {{a: {{b}}}} = getSource(); sink(b) must report the taint flow \
         (recursion reaches the nested leaf `b`)"
    );
}

/// #59: default value `const {token = 1} = getSource(); sink(token)` — the
/// `object_assignment_pattern` binds its left (the shorthand `token`); the
/// default `1` is not a binding.
#[test]
fn taint_destructuring_default() {
    use crate::taint::{TaintSink, TaintSource};
    use crate::taint_rules::TaintRule;

    let src = br#"function App() {
    const {token = 1} = getSource();
    sink(token);
}
"#;
    let rules = vec![TaintRule {
        id: "xss".into(),
        sources: vec![TaintSource {
            pattern: "getSource".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "sink".into(),
            arg_index: 0,
            cwe: "CWE-79".into(),
            description: "XSS".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }];

    let il = build_il(src, "typescript").unwrap();
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(analyze_taint(&cfg, &rules, "app.ts"));
    }
    assert!(
        !findings.is_empty(),
        "const {{token = 1}} = getSource(); sink(token) must report the taint flow \
         (object_assignment_pattern binds the left shorthand `token`)"
    );
}

/// #59 fail-safe preservation: the HARD LESSON from #55 is that a pattern /
/// non-identifier node's RAW TEXT must NEVER be used as a binding name (a
/// prior attempt coined a variable literally named "{token}" and poisoned the
/// name table). Now that `visit_var_decl` recurses patterns, the per-leaf
/// recursion must still bind ONLY leaf identifiers — never a raw pattern
/// fragment. This test builds IL for a mixed destructuring over a CLEAN rhs
/// (no source → no finding expected) and asserts NO lval ident in the IL
/// contains pattern punctuation (`{`, `}`, `[`, `]`, `...`, `:`), which would
/// indicate a decoy binding leaked from a non-identifier node's text. It also
/// confirms a TS type annotation on the declarator (`: {a: string}`) — a
/// sibling of the pattern, not a pattern child — does not poison bindings.
#[test]
fn taint_destructuring_no_decoy_binding_fail_safe() {
    use crate::il::{Base, Instr};
    use crate::taint::{TaintSink, TaintSource};
    use crate::taint_rules::TaintRule;

    // Clean rhs (no source) → no taint finding expected. The point of this
    // test is the decoy-binding invariant on the IL, not the taint result.
    let src = br#"function App() {
    const {a: b}: {a: string} = clean();
    const [{c}, d] = clean();
    const {...rest} = clean();
    sink(b);
    sink(c);
    sink(d);
    sink(rest);
}
"#;
    let rules = vec![TaintRule {
        id: "xss".into(),
        sources: vec![TaintSource {
            pattern: "getSource".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "sink".into(),
            arg_index: 0,
            cwe: "CWE-79".into(),
            description: "XSS".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }];

    let il = build_il(src, "typescript").unwrap();

    // (1) No decoy binding: no lval ident carries pattern punctuation.
    let mut decoys: Vec<String> = Vec::new();
    for func in &il.functions {
        for instr in &func.body {
            if let Instr::Assign { lval, .. } = instr
                && let Base::Var(name) = &lval.base
            {
                let ident = &name.ident;
                if ident.contains('{')
                    || ident.contains('}')
                    || ident.contains('[')
                    || ident.contains(']')
                    || ident.contains("...")
                    || ident.contains(':')
                {
                    decoys.push(ident.clone());
                }
            }
        }
    }
    assert!(
        decoys.is_empty(),
        "decoy binding leaked (raw pattern text used as a variable name): {decoys:?}"
    );

    // (2) No panic, no spurious finding (clean rhs, no source).
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(analyze_taint(&cfg, &rules, "app.ts"));
    }
    assert!(
        findings.is_empty(),
        "clean rhs destructuring must produce no finding; a finding would mean \
         a decoy binding collided with a sink arg: {findings:?}"
    );
}
