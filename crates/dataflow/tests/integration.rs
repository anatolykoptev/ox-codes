use ox_dataflow::{FindingKind, Finding, analysis, scope_walker};

fn analyze_source(source: &str, lang: &str, file: &str) -> Vec<Finding> {
    let chain = scope_walker::walk_file(source.as_bytes(), lang).unwrap();
    analysis::analyze(&chain, file)
}

fn has(findings: &[Finding], var: &str, kind: &FindingKind) -> bool {
    findings.iter().any(|f| f.variable == var && f.kind == *kind)
}

// --- Go tests ---

#[test]
fn go_dead_store() {
    let src = "package main\nimport \"fmt\"\nfunc foo() {\n\tx := 1\n\tx = 2\n\tfmt.Println(x)\n}";
    let f = analyze_source(src, "go", "test.go");
    assert!(has(&f, "x", &FindingKind::DeadStore), "x := 1 should be dead store, got: {f:?}");
}

#[test]
fn go_unused_variable() {
    let src = "package main\nimport \"fmt\"\nfunc foo() {\n\tx := 1\n\ty := 2\n\tfmt.Println(y)\n}";
    let f = analyze_source(src, "go", "test.go");
    assert!(has(&f, "x", &FindingKind::DeadStore), "x should be dead store, got: {f:?}");
    assert!(!has(&f, "y", &FindingKind::UnusedVariable), "y is used");
    assert!(!has(&f, "y", &FindingKind::DeadStore), "y is used");
}

#[test]
fn go_underscore_skip() {
    let src = "package main\nfunc foo() {\n\t_ = getValue()\n\t_unused := 1\n}";
    let f = analyze_source(src, "go", "test.go");
    let bad: Vec<_> = f.iter().filter(|f| {
        f.variable.starts_with('_')
            && (f.kind == FindingKind::DeadStore || f.kind == FindingKind::UnusedVariable)
    }).collect();
    assert!(bad.is_empty(), "_ and _unused should be skipped, got: {bad:?}");
}

#[test]
fn go_const_values() {
    let src = "package main\nfunc foo() {\n\tx := 42\n\ty := \"hello\"\n\tz := true\n}";
    let f = analyze_source(src, "go", "test.go");
    // All have constant values
    for var in ["x", "y", "z"] {
        assert!(has(&f, var, &FindingKind::ConstantValue), "{var} should have const value");
        assert!(has(&f, var, &FindingKind::DeadStore), "{var} should be dead store");
    }
}

// --- Python tests ---

#[test]
fn python_dead_store() {
    let src = "def foo():\n    x = 1\n    x = 2\n    print(x)\n";
    let f = analyze_source(src, "python", "test.py");
    assert!(has(&f, "x", &FindingKind::DeadStore), "x = 1 should be dead store, got: {f:?}");
}

#[test]
fn python_unused_variable() {
    let src = "def foo():\n    x = 1\n    y = 2\n    return y\n";
    let f = analyze_source(src, "python", "test.py");
    assert!(has(&f, "x", &FindingKind::DeadStore), "x should be dead store, got: {f:?}");
    assert!(!has(&f, "y", &FindingKind::UnusedVariable), "y is used");
}

// --- Edge cases ---

#[test]
fn go_empty_function() {
    let src = "package main\nfunc foo() {}";
    let f = analyze_source(src, "go", "test.go");
    let warnings: Vec<_> = f.iter()
        .filter(|f| f.kind == FindingKind::DeadStore || f.kind == FindingKind::UnusedVariable)
        .collect();
    assert!(warnings.is_empty(), "empty function should have no findings, got: {warnings:?}");
}

#[test]
fn go_multiple_functions() {
    let src = "package main\nimport \"fmt\"\nfunc foo() {\n\tx := 1\n\tfmt.Println(x)\n}\nfunc bar() {\n\ty := 2\n}";
    let f = analyze_source(src, "go", "test.go");
    // x in foo() is used
    assert!(!has(&f, "x", &FindingKind::DeadStore), "x in foo() is used");
    assert!(!has(&f, "x", &FindingKind::UnusedVariable), "x in foo() is used");
    // y in bar() is unused
    assert!(has(&f, "y", &FindingKind::DeadStore), "y in bar() should be dead store, got: {f:?}");
}

// --- Svelte template edge extraction tests ---


fn var_has_use(src: &str, lang: &str, name: &str) -> bool {
    let chain = ox_dataflow::scope_walker::walk_file(src.as_bytes(), lang).unwrap();
    chain.scopes.iter().any(|s| {
        s.vars.iter().any(|v| v.name == name && !v.uses.is_empty())
    })
}

/// on:click={handler} — handler should be referenced in template scope.
#[test]
fn svelte_on_click_handler_resolved() {
    let src = "<script>\nfunction handleClick() {}\n</script>\n<button on:click={handleClick}>click me</button>";
    assert!(
        var_has_use(src, "svelte", "handleClick"),
        "handleClick should be referenced via on:click template directive"
    );
}

/// use:drag (no expression) — action name itself is the reference.
#[test]
fn svelte_use_action_resolved() {
    let src = "<script>\nfunction drag(node) { return {}; }\n</script>\n<div use:drag>draggable</div>";
    assert!(
        var_has_use(src, "svelte", "drag"),
        "drag should be referenced via use:drag directive"
    );
}

/// {compute()} template expression — compute should be referenced.
#[test]
fn svelte_template_expression_resolved() {
    let src = "<script>\nfunction compute() { return 1; }\n</script>\n<p>{compute()}</p>";
    assert!(
        var_has_use(src, "svelte", "compute"),
        "{{compute()}} should be referenced via template expression"
    );
}

/// {#each items as item} — items should be referenced as the iterable.
#[test]
fn svelte_each_iter_resolved() {
    let src = "<script>\nlet items = [1, 2, 3];\n</script>\n{#each items as item}\n  <p>{item}</p>\n{/each}";
    assert!(
        var_has_use(src, "svelte", "items"),
        "items should be referenced in each iterable expression"
    );
}
