use super::walk_file;

#[test]
fn go_basic_uses() {
    let src = b"package main\nfunc foo() { x := 1; y := x + 2; _ = y }";
    let chain = walk_file(src, "go").unwrap();
    let x = find_var(&chain, "x");
    let y = find_var(&chain, "y");
    assert!(x.is_some(), "x should be found");
    assert!(y.is_some(), "y should be found");
    assert!(
        !x.unwrap().uses.is_empty(),
        "x should have uses (read by y := x + 2)"
    );
    assert!(
        !y.unwrap().uses.is_empty(),
        "y should have uses (read by _ = y)"
    );
}

#[test]
fn go_reassignment() {
    let src = b"package main\nfunc foo() { x := 1; x = 2; _ = x }";
    let chain = walk_file(src, "go").unwrap();
    // x should have two bindings (declaration + assignment).
    let xs: Vec<_> = chain
        .scopes
        .iter()
        .flat_map(|s| s.vars.iter())
        .filter(|v| v.name == "x")
        .collect();
    assert!(
        xs.len() >= 2,
        "x should have at least 2 bindings, got {}",
        xs.len()
    );
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

// ---------------------------------------------------------------------------
// Svelte span offset tests
// ---------------------------------------------------------------------------

/// Spans recorded during secondary-parse must be relative to the whole file,
/// not to the sub-slice passed to the TS parser.
///
/// Source layout:
///   `<script>function fn(){}</script><button on:click={fn}>`
///   0        8                       33      41      50
///
/// `fn` declaration is at offset 17 inside the file (after `<script>`).
/// `fn` reference inside `{fn}` is at offset 50 inside the file (after `{`).
/// Both start_byte values must be > 8 (the `<script>` tag length),
/// proving they are NOT relative to the sub-slice (which would give ~8 and ~0).
#[test]
fn svelte_span_offsets_are_relative_to_file() {
    // Build source and locate exact bytes.
    let src = b"<script>function fn(){}</script><button on:click={fn}>";
    // Find where "fn" first appears (the declaration inside <script>).
    let decl_offset = src.windows(2).position(|w| w == b"fn").unwrap();
    // Find where "{fn}" appears (the reference in the template).
    let brace_pos = src.iter().rposition(|&b| b == b'{').unwrap();
    let ref_offset = brace_pos + 1; // byte right after '{'

    let chain = walk_file(src, "svelte").unwrap();

    // Declaration span.
    let fn_binding = find_var(&chain, "fn");
    assert!(
        fn_binding.is_some(),
        "fn binding must be found in svelte script"
    );
    let binding = fn_binding.unwrap();
    assert_eq!(
        binding.def_site.start_byte, decl_offset,
        "fn def_site.start_byte ({}) must equal file-relative offset ({})",
        binding.def_site.start_byte, decl_offset
    );

    // Reference span.
    assert!(
        !binding.uses.is_empty(),
        "fn must have at least one use (on:click={{fn}})"
    );
    let use_span = binding.uses[0];
    assert_eq!(
        use_span.start_byte, ref_offset,
        "fn use start_byte ({}) must equal file-relative offset ({})",
        use_span.start_byte, ref_offset
    );
}

/// When a `<script>` block appears AFTER the template, directives that
/// reference script-declared functions should still resolve.
///
/// This is the two-pass ordering issue. Currently, resolution may silently
/// fail (no edge added). This test is expected to FAIL until a two-pass
/// walk (or deferred-reference resolution) is implemented.
///
/// Tracked as: feat/svelte-two-pass-ordering followup.
#[test]
fn svelte_script_after_template() {
    let src = b"<button on:click={fn}></button><script>function fn(){}</script>";
    let chain = walk_file(src, "svelte").unwrap();
    let fn_binding = find_var(&chain, "fn");
    assert!(fn_binding.is_some(), "fn binding must be found");
    assert!(
        !fn_binding.unwrap().uses.is_empty(),
        "fn must have a use edge from on:click={{fn}} even when script is after template"
    );
}

/// Smoke test: compiling and running with 50 expression nodes does not panic.
/// Validates that parser reuse across expressions works correctly.
#[test]
fn svelte_parser_reused_across_expressions() {
    // Build a svelte file with many {expr} nodes to exercise parser reuse.
    let mut src = String::from("<script>let x = 0;</script>");
    for i in 0..50_u32 {
        src.push_str(&format!("<span>{{x + {i}}}</span>"));
    }
    let result = walk_file(src.as_bytes(), "svelte");
    assert!(
        result.is_ok(),
        "walk_file must not panic with many expressions: {:?}",
        result.err()
    );
    let chain = result.unwrap();
    let x = find_var(&chain, "x");
    assert!(x.is_some(), "x declared in script must be found");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find the first VarBinding by name across all scopes.
fn find_var<'a>(
    chain: &'a crate::types::ScopeChain,
    name: &str,
) -> Option<&'a crate::types::VarBinding> {
    chain
        .scopes
        .iter()
        .flat_map(|s| s.vars.iter())
        .find(|v| v.name == name)
}
