use ox_dataflow::cfg_builder::build_cfg;
use ox_dataflow::il_builder::build_il;
use ox_dataflow::taint::{TaintFinding, TaintSink, TaintSource, analyze_taint};
use ox_dataflow::taint_rules::{TaintRule, default_rules};

fn analyze_taint_source(source: &str, lang: &str) -> Vec<TaintFinding> {
    let il = build_il(source.as_bytes(), lang).unwrap();
    let rules = default_rules(lang);
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(analyze_taint(&cfg, &rules, "test.go"));
    }
    findings
}

fn analyze_with_rules(source: &str, lang: &str, rules: &[TaintRule]) -> Vec<TaintFinding> {
    let il = build_il(source.as_bytes(), lang).unwrap();
    let mut findings = Vec::new();
    for func in &il.functions {
        let cfg = build_cfg(func);
        findings.extend(analyze_taint(&cfg, rules, "test.go"));
    }
    findings
}

fn sql_injection_rule_with_query_sink() -> TaintRule {
    TaintRule {
        id: "sql-injection".into(),
        sources: vec![TaintSource {
            pattern: "FormValue".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "Query".into(),
            arg_index: 0,
            cwe: "CWE-89".into(),
            description: "SQL injection".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }
}

fn cmd_injection_rule() -> TaintRule {
    TaintRule {
        id: "command-injection".into(),
        sources: vec![TaintSource {
            pattern: "FormValue".into(),
            tag: "user_input".into(),
        }],
        sinks: vec![TaintSink {
            pattern: "Command".into(),
            arg_index: -1,
            cwe: "CWE-78".into(),
            description: "OS command injection".into(),
        }],
        sanitizers: vec![],
        severity: "error".into(),
    }
}

// --- Go tests ---

#[test]
fn go_sql_injection_detected() {
    let src = r#"package main
func handler() {
    input := FormValue("name")
    Query(input)
}"#;
    let rules = vec![sql_injection_rule_with_query_sink()];
    let findings = analyze_with_rules(src, "go", &rules);
    assert!(
        !findings.is_empty(),
        "should detect SQL injection: {findings:?}"
    );
    assert!(
        findings.iter().any(|f| f.sink.cwe == "CWE-89"),
        "should have CWE-89 finding"
    );
}

#[test]
fn go_command_injection_detected() {
    let src = r#"package main
func handler() {
    input := FormValue("cmd")
    Command(input)
}"#;
    let rules = vec![cmd_injection_rule()];
    let findings = analyze_with_rules(src, "go", &rules);
    assert!(
        !findings.is_empty(),
        "should detect command injection: {findings:?}"
    );
    assert!(
        findings.iter().any(|f| f.sink.cwe == "CWE-78"),
        "should have CWE-78 finding"
    );
}

#[test]
fn go_no_taint_without_source() {
    let src = r#"package main
func handler() {
    x := "safe"
    Query(x)
}"#;
    let rules = vec![sql_injection_rule_with_query_sink()];
    let findings = analyze_with_rules(src, "go", &rules);
    assert!(
        findings.is_empty(),
        "string literal is not a taint source: {findings:?}"
    );
}

#[test]
fn go_taint_propagation_through_assignment() {
    let src = r#"package main
func handler() {
    input := FormValue("name")
    query := "SELECT * FROM users WHERE name = " + input
    Query(query)
}"#;
    let rules = vec![sql_injection_rule_with_query_sink()];
    let findings = analyze_with_rules(src, "go", &rules);
    assert!(
        !findings.is_empty(),
        "should detect taint via propagation through query: {findings:?}"
    );
    assert!(
        findings.iter().any(|f| f.sink.cwe == "CWE-89"),
        "should have CWE-89"
    );
}

#[test]
fn go_default_rules_with_exec_sink() {
    // Uses default rules which have Exec as sink
    let src = r#"package main
func handler() {
    input := FormValue("name")
    Exec(input)
}"#;
    let findings = analyze_taint_source(src, "go");
    assert!(
        !findings.is_empty(),
        "default Go rules should detect FormValue -> Exec: {findings:?}"
    );
    assert!(findings.iter().any(|f| f.sink.cwe == "CWE-89"));
}

// --- Python tests ---

#[test]
fn python_sql_injection_detected() {
    let src = "def handler():\n    user_input = input(\"name\")\n    execute(user_input)\n";
    let findings = analyze_taint_source(src, "python");
    assert!(
        !findings.is_empty(),
        "should detect Python SQL injection: {findings:?}"
    );
    assert!(
        findings.iter().any(|f| f.sink.cwe == "CWE-89"),
        "should have CWE-89 finding"
    );
}

// --- Rule coverage tests ---

#[test]
fn default_go_rules_have_multiple_rules() {
    let rules = default_rules("go");
    assert!(
        rules.len() >= 2,
        "Go should have at least 2 rules, got {}",
        rules.len()
    );
}

#[test]
fn default_python_rules_have_at_least_one() {
    let rules = default_rules("python");
    assert!(
        !rules.is_empty(),
        "Python should have at least 1 rule, got {}",
        rules.len()
    );
}

#[test]
fn unknown_language_returns_no_rules() {
    assert!(default_rules("brainfuck").is_empty());
}
