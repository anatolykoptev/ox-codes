//! Hard red tests: solver, reaching defs, def-use chains, taint, CFG analysis.

use ox_dataflow::cfg_builder::build_cfg;
use ox_dataflow::def_use::build_def_use_chains;
use ox_dataflow::il_builder::build_il;
use ox_dataflow::reaching_defs::collect_definitions;
use ox_dataflow::taint::{Sanitizer, TaintFinding, TaintSink, TaintSource, analyze_taint};
use ox_dataflow::taint_rules::{TaintRule, default_rules};
use ox_dataflow::types::FindingKind;

fn go_cfg(s: &str) -> ox_dataflow::cfg::Cfg {
    build_cfg(&build_il(s.as_bytes(), "go").unwrap().functions[0])
}
fn go_taint(s: &str, r: &[TaintRule]) -> Vec<TaintFinding> {
    let il = build_il(s.as_bytes(), "go").unwrap();
    il.functions
        .iter()
        .flat_map(|f| analyze_taint(&build_cfg(f), r, "t.go"))
        .collect()
}
fn go_findings(s: &str) -> Vec<ox_dataflow::types::Finding> {
    ox_dataflow::cfg_analysis::analyze_cfg(s.as_bytes(), "go", "t.go")
}
fn sqli_rule() -> TaintRule {
    TaintRule {
        id: "sqli".into(),
        sources: vec![TaintSource {
            pattern: "FormValue".into(),
            tag: "i".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "Query".into(),
            arg_index: 0,
            cwe: "CWE-89".into(),
            description: "sqli".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }
}

// --- Reaching Defs / Solver ---

#[test]
fn redef_same_block_only_latest_reaches_use() {
    let c =
        build_def_use_chains(&go_cfg("package main\nfunc f(){x:=1;x=2;fmt.Println(x)}")).unwrap();
    let x: Vec<_> = c.iter().filter(|c| c.def.name.ident == "x").collect();
    assert!(x.len() >= 2);
    let first = x.iter().min_by_key(|c| c.def.span.start_byte).unwrap();
    assert!(
        first.uses.is_empty(),
        "x:=1 killed by x=2: {:?}",
        first.uses
    );
}

#[test]
fn both_branch_defs_collected() {
    let src = "package main\nfunc f(){\nif true{x:=1;_=x}else{x:=2;_=x}\n}";
    let n = collect_definitions(&go_cfg(src))
        .iter()
        .filter(|d| d.name.ident == "x")
        .count();
    assert!(n >= 2, "expected defs from both branches, got {n}");
}

#[test]
fn unused_var_empty_uses() {
    let c = build_def_use_chains(&go_cfg("package main\nfunc f(){x:=42}")).unwrap();
    assert!(
        c.iter()
            .find(|c| c.def.name.ident == "x")
            .unwrap()
            .uses
            .is_empty()
    );
}

#[test]
fn var_used_twice_both_captured() {
    let c = build_def_use_chains(&go_cfg("package main\nfunc f(){x:=1;y:=x+x;_=y}")).unwrap();
    assert!(
        c.iter()
            .find(|c| c.def.name.ident == "x")
            .unwrap()
            .uses
            .len()
            >= 2
    );
}

#[test]
fn parameter_in_il_params() {
    let il = build_il(b"package main\nfunc f(x int){_=x}", "go").unwrap();
    assert!(il.functions[0].params.iter().any(|p| p.ident == "x"));
}

// --- Def-Use Chains ---

#[test]
fn self_assignment_first_def_used() {
    let c = build_def_use_chains(&go_cfg("package main\nfunc f(){x:=1;x=x+1;_=x}")).unwrap();
    let x: Vec<_> = c.iter().filter(|c| c.def.name.ident == "x").collect();
    assert!(x.len() >= 2);
    assert!(
        x.iter().any(|c| !c.uses.is_empty()),
        "first x def used in x=x+1 RHS"
    );
}

#[test]
fn transitive_chain_x_to_y_to_z() {
    let c = build_def_use_chains(&go_cfg("package main\nfunc f(){x:=1;y:=x;z:=y;_=z}")).unwrap();
    assert!(
        !c.iter()
            .find(|c| c.def.name.ident == "x")
            .unwrap()
            .uses
            .is_empty()
    );
    assert!(
        !c.iter()
            .find(|c| c.def.name.ident == "y")
            .unwrap()
            .uses
            .is_empty()
    );
}

#[test]
fn no_defs_empty_chains() {
    assert!(
        build_def_use_chains(&go_cfg("package main\nfunc f(){}"))
            .unwrap()
            .is_empty()
    );
}

// --- Taint ---

#[test]
fn taint_via_string_concat() {
    let s = "package main\nfunc f(){\ninput:=FormValue(\"x\")\nq:=\"SELECT \"+input\nQuery(q)\n}";
    assert!(
        !go_taint(s, &[sqli_rule()]).is_empty(),
        "concat propagates taint"
    );
}

#[test]
fn direct_nested_source_in_sink_no_panic() {
    let _ = go_taint(
        "package main\nfunc f(){Query(FormValue(\"x\"))}",
        &[sqli_rule()],
    );
}

#[test]
fn multiple_sources_all_reach_sinks() {
    let r = TaintRule {
        id: "m".into(),
        sources: vec![
            TaintSource {
                pattern: "FormValue".into(),
                tag: "a".into(),
            },
            TaintSource {
                pattern: "ReadAll".into(),
                tag: "b".into(),
            },
        ],
        sinks: vec![TaintSink {
            pattern: "Query".into(),
            arg_index: 0,
            cwe: "CWE-89".into(),
            description: "s".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    };
    let s = "package main\nfunc f(){\na:=FormValue(\"x\")\nb:=ReadAll(body)\nQuery(a)\nQuery(b)\n}";
    assert!(
        go_taint(s, &[r]).len() >= 2,
        "both sources should reach sinks"
    );
}

#[test]
fn same_func_source_and_sink() {
    let r = TaintRule {
        id: "s".into(),
        sources: vec![TaintSource {
            pattern: "Query".into(),
            tag: "d".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "Query".into(),
            arg_index: 0,
            cwe: "CWE-89".into(),
            description: "s".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    };
    let s = "package main\nfunc f(){\nresult:=Query(\"safe\")\nQuery(result)\n}";
    assert!(!go_taint(s, &[r]).is_empty(), "feedback loop detected");
}

#[test]
fn underscore_taint_no_panic() {
    let _ = go_taint(
        "package main\nfunc f(){_=FormValue(\"x\");Query(_)}",
        &[sqli_rule()],
    );
}

#[test]
fn custom_rules_not_defaults() {
    let c = TaintRule {
        id: "custom-xss".into(),
        sources: vec![TaintSource {
            pattern: "UserInput".into(),
            tag: "c".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "Render".into(),
            arg_index: 0,
            cwe: "CWE-79".into(),
            description: "x".into(),
        }],
        sanitizers: vec![Sanitizer {
            pattern: "Escape".into(),
        }],
        severity: "warning".into(),
    };
    let s = "package main\nfunc f(){\ndata:=UserInput(\"f\")\nRender(data)\n}";
    let f = go_taint(s, &[c]);
    assert!(!f.is_empty() && f[0].rule_id == "custom-xss");
    assert!(
        go_taint(s, &default_rules("go")).is_empty(),
        "defaults shouldn't match"
    );
}

// --- CFG Analysis ---

#[test]
fn dead_store_overwrite() {
    let f = go_findings("package main\nfunc f(){x:=1;x=2;fmt.Println(x)}");
    assert!(
        f.iter()
            .any(|f| f.kind == FindingKind::DeadStore && f.variable == "x"),
        "{f:?}"
    );
}

#[test]
fn used_in_condition_not_dead() {
    let f = go_findings("package main\nfunc f(){\nx:=1\nif x>0{fmt.Println(x)}\n}");
    assert!(
        !f.iter()
            .any(|f| f.kind == FindingKind::DeadStore && f.variable == "x"),
        "{f:?}"
    );
}

#[test]
fn param_not_flagged_dead_store() {
    let f = go_findings("package main\nfunc f(x int){y:=1;fmt.Println(y)}");
    assert!(
        !f.iter()
            .any(|f| f.kind == FindingKind::DeadStore && f.variable == "x"),
        "{f:?}"
    );
}

#[test]
fn underscore_prefix_skipped() {
    let f = go_findings("package main\nfunc f(){_unused:=getValue()}");
    assert!(
        !f.iter()
            .any(|f| f.kind == FindingKind::DeadStore && f.variable.starts_with('_'))
    );
}
