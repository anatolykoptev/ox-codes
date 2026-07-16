#[cfg(test)]
use super::REPARSE_COUNT;
use super::{walk_file, walk_file_with_ext};

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
// TSX grammar selection (issue #44)
// ---------------------------------------------------------------------------

/// A `.tsx` file parsed with the non-JSX TypeScript grammar produces ERROR
/// nodes wherever JSX appears, silently dropping any binding or reference
/// inside (or downstream of) a JSX block.  Here `secret` is declared by a
/// `const` and used inside a JSX expression container (`{secret}`).  With the
/// TSX grammar the `secret` inside `{secret}` is an `identifier` node and is
/// captured as a use; with the non-JSX grammar it is lost.
#[test]
fn tsx_jsx_reference_not_dropped() {
    let src = br#"function App() {
    const secret = getSecret();
    return <div>{secret}</div>;
}
"#;
    let chain = walk_file_with_ext(src, "typescript", "tsx").unwrap();
    let secret = find_var(&chain, "secret");
    assert!(secret.is_some(), "secret binding must be found");
    let secret = secret.unwrap();
    assert!(
        !secret.uses.is_empty(),
        "secret must have at least one use (inside JSX {{secret}}), got {} uses",
        secret.uses.len()
    );
}

// ---------------------------------------------------------------------------
// .js-with-JSX grammar fallback (issue #58)
// ---------------------------------------------------------------------------

/// A `.js` file containing JSX (legacy React) is sent to the non-JSX
/// `LANGUAGE_TYPESCRIPT` grammar by the extension heuristic. The non-JSX
/// grammar produces ERROR nodes on JSX, silently dropping bindings/refs
/// inside JSX. After the #58 fix (parse-and-detect-ERROR fallback to TSX),
/// the JSX-embedded reference `{secret}` must be captured.
#[test]
fn js_with_jsx_reference_not_dropped() {
    let src = br#"function App() {
    const secret = getSecret();
    return <div>{secret}</div>;
}
"#;
    let chain = walk_file_with_ext(src, "typescript", "js").unwrap();
    let secret = find_var(&chain, "secret");
    assert!(
        secret.is_some(),
        "secret binding must be found in .js-with-JSX"
    );
    let secret = secret.unwrap();
    assert!(
        !secret.uses.is_empty(),
        "secret must have at least one use (inside JSX {{secret}}), got {} uses",
        secret.uses.len()
    );
}

/// Perf gate (issue #58): a clean `.js` file (no JSX, no parse errors) must
/// NOT trigger the TSX re-parse — the common case pays zero double-parse
/// overhead. Verified via the `REPARSE_COUNT` thread-local counter.
#[test]
fn clean_js_no_reparse() {
    REPARSE_COUNT.with(|c| c.set(0));
    let src = br#"function App() {
    const secret = getSecret();
    return secret + 1;
}
"#;
    let chain = walk_file_with_ext(src, "typescript", "js").unwrap();
    assert_eq!(
        REPARSE_COUNT.with(|c| c.get()),
        0,
        "clean .js (no JSX, no errors) must not trigger the TSX re-parse (perf gate)"
    );
    let secret = find_var(&chain, "secret").unwrap();
    assert!(
        !secret.uses.is_empty(),
        "secret should still have uses in a clean .js file"
    );
}

/// Perf gate (issue #58): a clean `.ts` file must also NOT trigger the
/// re-parse.
#[test]
fn clean_ts_no_reparse() {
    REPARSE_COUNT.with(|c| c.set(0));
    let src = br#"function add(a: number, b: number): number {
    return a + b;
}
"#;
    let chain = walk_file_with_ext(src, "typescript", "ts").unwrap();
    assert_eq!(
        REPARSE_COUNT.with(|c| c.get()),
        0,
        "clean .ts (no JSX, no errors) must not trigger the TSX re-parse (perf gate)"
    );
    let a = find_var(&chain, "a").unwrap();
    assert!(!a.uses.is_empty(), "param a should have uses");
}

/// Regression guard (issue #58): `.tsx` files already use the TSX grammar via
/// the extension heuristic — the #58 fallback must NOT fire for them (no
/// double re-parse), and JSX-embedded refs must still be captured.
#[test]
fn tsx_does_not_trigger_fallback() {
    REPARSE_COUNT.with(|c| c.set(0));
    let src = br#"function App() {
    const secret = getSecret();
    return <div>{secret}</div>;
}
"#;
    let chain = walk_file_with_ext(src, "typescript", "tsx").unwrap();
    assert_eq!(
        REPARSE_COUNT.with(|c| c.get()),
        0,
        ".tsx already uses TSX grammar — #58 fallback must not trigger"
    );
    let secret = find_var(&chain, "secret").unwrap();
    assert!(
        !secret.uses.is_empty(),
        "secret use in JSX must be captured (unchanged .tsx handling)"
    );
}

/// The #58 fallback DOES fire for a JSX-bearing `.js` file — confirmed via
/// the `REPARSE_COUNT` counter — and recovers the JSX-embedded binding.
#[test]
fn js_with_jsx_triggers_reparse() {
    REPARSE_COUNT.with(|c| c.set(0));
    let src = br#"function App() {
    const secret = getSecret();
    return <div>{secret}</div>;
}
"#;
    let chain = walk_file_with_ext(src, "typescript", "js").unwrap();
    assert!(
        REPARSE_COUNT.with(|c| c.get()) > 0,
        "JSX-bearing .js must trigger the TSX re-parse"
    );
    let secret = find_var(&chain, "secret").unwrap();
    assert!(
        !secret.uses.is_empty(),
        "secret use in JSX must be captured after re-parse"
    );
}

// ---------------------------------------------------------------------------
// walk_as_typescript save/restore (issue #49)
// ---------------------------------------------------------------------------

/// `walk_as_typescript` must save AND restore `ctx.is_ts_secondary` (symmetric
/// with `is_svelte`/`span_offset`).  Today it hardcodes `false` on exit, so a
/// nested call clobbers the outer frame's flag.  Pre-set `is_ts_secondary =
/// true`, call the smallest reachable wrapper, and assert it is STILL true.
#[test]
fn walk_as_typescript_preserves_is_ts_secondary() {
    let ts_lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let q = super::build_ts_queries(&ts_lang).unwrap();
    let mut ts_parser = tree_sitter::Parser::new();
    ts_parser.set_language(&ts_lang).unwrap();

    let mut ctx = super::WalkCtx {
        sid: 0,
        finished: Vec::new(),
        is_svelte: false,
        is_ts_secondary: true, // pre-set — must survive the call
        ts_parser,
        span_offset: 0,
        pending_refs: Vec::new(),
    };
    let mut stack: Vec<crate::types::Scope> = Vec::new();
    super::walk_as_typescript(b"hello", 0, &q, &mut stack, &mut ctx);
    assert!(
        ctx.is_ts_secondary,
        "is_ts_secondary must be restored to true after walk_as_typescript returns"
    );
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
