use anyhow::{Context, Result};
use smallvec::SmallVec;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::queries;
use crate::types::{ConstValue, Scope, ScopeChain, ScopeKind, Span, VarBinding};

/// Walk a source file and build a scope chain with variable bindings.
///
/// Delegates to [`walk_file_with_ext`] with an empty extension — callers that
/// know the file extension (and thus whether to use the TSX grammar for
/// `.tsx`/`.jsx`) should call that directly.
pub fn walk_file(source: &[u8], lang_name: &str) -> Result<ScopeChain> {
    walk_file_with_ext(source, lang_name, "")
}

/// Walk a source file and build a scope chain with variable bindings.
///
/// `file_ext` (without leading dot, e.g. `"tsx"`) is used to select the
/// tree-sitter grammar for the TypeScript family: `.tsx`/`.jsx` files are
/// parsed with `LANGUAGE_TSX` (the JSX-aware grammar) instead of the
/// non-JSX `LANGUAGE_TYPESCRIPT`, which produces `ERROR` nodes on JSX and
/// silently drops bindings/refs in or after JSX blocks.
pub fn walk_file_with_ext(source: &[u8], lang_name: &str, file_ext: &str) -> Result<ScopeChain> {
    let lq = queries::get_queries(lang_name)
        .with_context(|| format!("unsupported language: {lang_name}"))?;
    // For the TypeScript family, .tsx/.jsx files must use the JSX-aware TSX
    // grammar; the non-JSX LANGUAGE_TYPESCRIPT produces ERROR nodes on JSX,
    // silently dropping bindings/refs in or after JSX blocks.  Grammar
    // selection goes through ox_langs::effective_language_id + get_language
    // (the single source of truth) instead of an inline LANGUAGE_TSX literal.
    let effective_id = ox_langs::effective_language_id(lang_name, file_ext);
    let mut lang: tree_sitter::Language = effective_id
        .and_then(|id| ox_langs::get_language(id).map(|c| c.language))
        .unwrap_or_else(|| lq.language().clone());
    let mut parser = Parser::new();
    parser.set_language(&lang)?;
    let mut tree = parser.parse(source, None).context("parse failed")?;

    // Issue #58: .js/.ts files may contain JSX (legacy React). The extension
    // heuristic selects the non-JSX LANGUAGE_TYPESCRIPT, which produces ERROR
    // nodes on JSX and silently drops JSX-embedded bindings/refs — the same
    // failure class as #44 but for .js-with-JSX. When the non-JSX parse has
    // ERROR/MISSING nodes, re-parse with the JSX-aware LANGUAGE_TSX and keep
    // whichever yields fewer errors.
    //
    // Perf gate: the re-parse is CONDITIONAL — only when the first parse
    // actually has errors (detected via has_error(), then is_error()/
    // is_missing() for counting — NOT kind() string compares, the #44/#53
    // lesson). A clean .js/.ts file (the common case) has zero errors → no
    // re-parse → zero overhead.
    if effective_id == Some("typescript") && tree.root_node().has_error() {
        let tsx_lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TSX.into();
        let mut tsx_parser = Parser::new();
        if tsx_parser.set_language(&tsx_lang).is_ok()
            && let Some(tsx_tree) = tsx_parser.parse(source, None)
        {
            let non_jsx_errs = count_errors(tree.root_node());
            let tsx_errs = count_errors(tsx_tree.root_node());
            #[cfg(test)]
            REPARSE_COUNT.with(|c| c.set(c.get() + 1));
            if tsx_errs < non_jsx_errs {
                lang = tsx_lang;
                tree = tsx_tree;
            }
        }
    }

    let ts_lang = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let ts_queries = build_ts_queries(&ts_lang)?;

    // For Svelte files we drive the walk with the Svelte tree and secondary-parse
    // TypeScript sub-trees; for all other languages we use the language's own
    // queries for declarations / assignments / parameters.
    let is_svelte = lang_name == "svelte";

    let compiled = if is_svelte {
        // Svelte grammar has no TS-level decl/assign/param nodes; stubs satisfy
        // the type but produce zero matches.  Real work is in secondary parsing.
        ts_queries
    } else {
        CompiledQueries {
            decl: Query::new(&lang, lq.declarations_query())?,
            assign: Query::new(&lang, lq.assignments_query())?,
            param: Query::new(&lang, lq.parameters_query())?,
        }
    };

    // Build a reusable TS parser for secondary parses (avoids per-expression alloc).
    let mut ts_parser = Parser::new();
    let ts_lang_for_reuse: tree_sitter::Language =
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    // Ignore error — if this fails, walk_as_typescript will handle gracefully.
    let _ = ts_parser.set_language(&ts_lang_for_reuse);

    let mut ctx = WalkCtx {
        sid: 0,
        finished: Vec::new(),
        is_svelte,
        is_ts_secondary: false,
        ts_parser,
        span_offset: 0,
        pending_refs: Vec::new(),
    };
    let mut stack = vec![make_scope(ScopeKind::Module, tree.root_node(), 0)];
    walk_node(tree.root_node(), source, &compiled, &mut stack, &mut ctx);
    // Resolve deferred references (Svelte template refs processed before the
    // <script> block's declarations entered scope) against the final scope
    // stack. Only still-open (enclosing) scopes are searched, so refs to
    // already-popped function-local bindings are NOT falsely resolved.
    resolve_pending_refs(&ctx.pending_refs, &mut stack);
    while let Some(s) = stack.pop() {
        ctx.finished.push(s);
    }
    Ok(ScopeChain {
        scopes: ctx.finished,
    })
}

/// Count ERROR and MISSING nodes in a tree using the tree-sitter node API
/// (`is_error()` / `is_missing()`), NOT `kind()` string compares (the
/// #44/#53 lesson). Used to pick the parse with fewer errors when falling
/// back from the non-JSX grammar to `LANGUAGE_TSX` for `.js`/`.ts` files
/// containing JSX.
fn count_errors(node: Node) -> usize {
    let mut count = if node.is_error() || node.is_missing() {
        1
    } else {
        0
    };
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            count += count_errors(cursor.node());
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    count
}

// Test-only thread-local counter: incremented each time the #58 ERROR-fallback
// triggers a TSX re-parse. Tests use this to assert the perf gate (clean
// `.js`/`.ts` → counter stays 0 → no re-parse) and to confirm the fallback
// fires for JSX-bearing `.js`.
#[cfg(test)]
thread_local! {
    pub(crate) static REPARSE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Build CompiledQueries for TypeScript (used for Svelte secondary parses).
fn build_ts_queries(ts_lang: &tree_sitter::Language) -> Result<CompiledQueries> {
    use crate::queries::LangQueries;
    use crate::queries::typescript::TypescriptQueries;
    let tq = TypescriptQueries::new();
    Ok(CompiledQueries {
        decl: Query::new(ts_lang, tq.declarations_query())?,
        assign: Query::new(ts_lang, tq.assignments_query())?,
        param: Query::new(ts_lang, tq.parameters_query())?,
    })
}

/// Secondary-parse a UTF-8 slice as TypeScript and walk it, recording
/// references and bindings into the current scope stack.
///
/// `offset` is the byte position of `raw` within the original source file.
/// All spans produced during this walk are shifted by `offset` so they refer
/// to the original file coordinates, not to the sub-slice.
fn walk_as_typescript(
    raw: &[u8],
    offset: usize,
    q: &CompiledQueries,
    stack: &mut Vec<Scope>,
    ctx: &mut WalkCtx,
) {
    // Reuse the parser stored in ctx — no per-call alloc.
    let Some(tree) = ctx.ts_parser.parse(raw, None) else {
        return;
    };
    // Walk the secondary tree; mark as non-svelte so we use compiled TS queries.
    let saved_svelte = ctx.is_svelte;
    let saved_offset = ctx.span_offset;
    let saved_ts_secondary = ctx.is_ts_secondary;
    ctx.is_svelte = false;
    ctx.is_ts_secondary = true;
    ctx.span_offset = offset;
    walk_node(tree.root_node(), raw, q, stack, ctx);
    ctx.is_svelte = saved_svelte;
    ctx.is_ts_secondary = saved_ts_secondary;
    ctx.span_offset = saved_offset;
}

struct CompiledQueries {
    decl: Query,
    assign: Query,
    param: Query,
}
struct WalkCtx {
    sid: u32,
    finished: Vec<Scope>,
    is_svelte: bool,
    is_ts_secondary: bool,
    /// Reusable TypeScript parser (avoids Parser::new() per expression node).
    ts_parser: Parser,
    /// Byte offset to add to all spans during a secondary-parse walk.
    /// Equals child.start_byte() in the original Svelte source.
    span_offset: usize,
    /// References whose target binding was not yet in scope when encountered
    /// (Svelte template refs processed before the `<script>` block). Resolved
    /// against the final scope stack after the walk completes.
    pending_refs: Vec<PendingRef>,
}

/// A reference deferred because its target wasn't in scope yet at the point
/// the reference was walked (Svelte script-after-template ordering).
struct PendingRef {
    name: String,
    span: Span,
}

fn walk_node(
    node: Node,
    src: &[u8],
    q: &CompiledQueries,
    stack: &mut Vec<Scope>,
    ctx: &mut WalkCtx,
) {
    let kind = node.kind();

    if ctx.is_svelte {
        // --- Svelte-specific node handling ---
        match kind {
            // <script>…</script>: secondary-parse the raw_text child as TS
            "script_element" => {
                walk_svelte_script(node, src, q, stack, ctx);
                return; // children handled inside
            }
            // {expression} template tags
            "expression" => {
                walk_svelte_expression(node, src, q, stack, ctx);
                return;
            }
            // <element on:click={fn} use:action bind:value={v}>
            "attribute" => {
                walk_svelte_attribute(node, src, q, stack, ctx);
                return;
            }
            // {#if condition} — condition is in if_start > svelte_raw_text
            "if_start" => {
                walk_svelte_block_condition(node, "condition", src, q, stack, ctx);
            }
            // {#each items as item} — iterable expression in identifier field
            "each_start" => {
                walk_svelte_each_start(node, src, q, stack, ctx);
            }
            // {#await expr} — expression in svelte_raw_text child
            "await_start" | "catch_start" | "then_start" | "else_if_start" => {
                walk_svelte_raw_text_children(node, src, q, stack, ctx);
            }
            // {@render snippet(args)}
            "render_tag" => {
                walk_svelte_raw_text_children(node, src, q, stack, ctx);
                return;
            }
            _ => {}
        }
    } else {
        // --- Non-Svelte (or secondary-parse TS) handling ---
        let push = scope_kind_for(kind);
        if let Some(sk) = push {
            stack.push(make_scope(sk, node, ctx.span_offset));
            if sk == ScopeKind::Function {
                collect_params(node, src, &q.param, stack, &mut ctx.sid, ctx.span_offset);
            }
        }
        if is_decl_node(kind) {
            collect_bindings(
                node,
                src,
                &q.decl,
                stack,
                &mut ctx.sid,
                false,
                ctx.span_offset,
            );
        } else if is_assign_node(kind) {
            collect_bindings(
                node,
                src,
                &q.assign,
                stack,
                &mut ctx.sid,
                false,
                ctx.span_offset,
            );
        } else if ctx.is_ts_secondary && is_func_decl_node(kind) {
            // Bind the function name to the enclosing scope (not the function's own scope).
            bind_func_name(node, src, stack, &mut ctx.sid, ctx.span_offset);
        } else if node.child_count() == 0 && kind == "identifier" {
            record_reference(node, src, stack, ctx);
        }
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                walk_node(cursor.node(), src, q, stack, ctx);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
        if push.is_some()
            && let Some(s) = stack.pop()
        {
            ctx.finished.push(s);
        }
        return;
    }

    // For Svelte nodes not handled by early return above: recurse into children.
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_node(cursor.node(), src, q, stack, ctx);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

// --- Svelte helper functions ---

/// Walk `<script>` element: find `raw_text` child and secondary-parse as TS.
fn walk_svelte_script(
    node: Node,
    src: &[u8],
    q: &CompiledQueries,
    stack: &mut Vec<Scope>,
    ctx: &mut WalkCtx,
) {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let child = cursor.node();
        if child.kind() == "raw_text" {
            let raw = &src[child.byte_range()];
            let offset = child.start_byte();
            walk_as_typescript(raw, offset, q, stack, ctx);
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// Walk `expression` node: extract `svelte_raw_text` child and secondary-parse.
fn walk_svelte_expression(
    node: Node,
    src: &[u8],
    q: &CompiledQueries,
    stack: &mut Vec<Scope>,
    ctx: &mut WalkCtx,
) {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let child = cursor.node();
        if child.kind() == "svelte_raw_text" {
            let raw = &src[child.byte_range()];
            let offset = child.start_byte();
            walk_as_typescript(raw, offset, q, stack, ctx);
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// Walk `attribute` node: if attribute_name is a directive, secondary-parse
/// the expression child; otherwise walk normally.
fn walk_svelte_attribute(
    node: Node,
    src: &[u8],
    q: &CompiledQueries,
    stack: &mut Vec<Scope>,
    ctx: &mut WalkCtx,
) {
    use ox_langs::preproc::svelte_refs;

    // Find attribute_name and expression children
    let mut attr_name: Option<String> = None;
    let mut expressions: Vec<Node> = Vec::new();
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let child = cursor.node();
        match child.kind() {
            "attribute_name" => {
                attr_name = Some(text_of(child, src));
            }
            "expression" | "quoted_attribute_value" => {
                expressions.push(child);
            }
            _ => {}
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }

    // If it's a directive that carries an expression, secondary-parse each.
    if let Some(name) = &attr_name
        && let Some(directive) = svelte_refs::parse_directive(name)
    {
        use svelte_refs::DirectiveKind;
        match directive.kind {
            DirectiveKind::EventHandler
            | DirectiveKind::Action
            | DirectiveKind::Binding
            | DirectiveKind::Transition
            | DirectiveKind::Animation
            | DirectiveKind::Let => {
                for expr in &expressions {
                    walk_svelte_expression(*expr, src, q, stack, ctx);
                }
                // If no expression: the directive name itself IS the identifier
                // (e.g. `use:drag` with no `={...}`) — record it as a reference.
                if expressions.is_empty() {
                    record_name_as_reference(&directive.name, node, src, stack, ctx);
                }
            }
            DirectiveKind::Class | DirectiveKind::Style | DirectiveKind::Unknown => {
                // Boolean toggle directives — expression is the JS side.
                for expr in &expressions {
                    walk_svelte_expression(*expr, src, q, stack, ctx);
                }
            }
        }
        return;
    }
    // Plain attribute: walk expressions for completeness.
    for expr in &expressions {
        walk_svelte_expression(*expr, src, q, stack, ctx);
    }
}

/// Walk a block-start node that has a named field carrying the condition expr.
fn walk_svelte_block_condition(
    node: Node,
    field: &str,
    src: &[u8],
    q: &CompiledQueries,
    stack: &mut Vec<Scope>,
    ctx: &mut WalkCtx,
) {
    if let Some(cond) = node.child_by_field_name(field)
        && cond.kind() == "svelte_raw_text"
    {
        let raw = &src[cond.byte_range()];
        let offset = cond.start_byte();
        walk_as_typescript(raw, offset, q, stack, ctx);
    }
}

/// Walk `each_start` node: process the iterable expression (identifier field)
/// and record the binding (parameter field) as a declared variable.
fn walk_svelte_each_start(
    node: Node,
    src: &[u8],
    q: &CompiledQueries,
    stack: &mut Vec<Scope>,
    ctx: &mut WalkCtx,
) {
    // identifier field: the expression being iterated (e.g. `items`)
    if let Some(id_node) = node.child_by_field_name("identifier")
        && id_node.kind() == "svelte_raw_text"
    {
        let raw = &src[id_node.byte_range()];
        let offset = id_node.start_byte();
        walk_as_typescript(raw, offset, q, stack, ctx);
    }
    // parameter field: the binding name (e.g. `item`) — declare, don't reference
    if let Some(param_node) = node.child_by_field_name("parameter")
        && param_node.kind() == "svelte_raw_text"
    {
        let name = text_of(param_node, src).trim().to_string();
        if !name.is_empty() {
            ctx.sid += 1;
            if let Some(scope) = stack.last_mut() {
                scope
                    .vars
                    .push(new_binding(name, ctx.sid, param_node, None, true, 0));
            }
        }
    }
}

/// Walk all `svelte_raw_text` children of a node as TypeScript expressions.
fn walk_svelte_raw_text_children(
    node: Node,
    src: &[u8],
    q: &CompiledQueries,
    stack: &mut Vec<Scope>,
    ctx: &mut WalkCtx,
) {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let child = cursor.node();
        if child.kind() == "svelte_raw_text" {
            let raw = &src[child.byte_range()];
            let offset = child.start_byte();
            walk_as_typescript(raw, offset, q, stack, ctx);
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// Synthesize a zero-span reference from a plain name string.
fn record_name_as_reference(
    name: &str,
    node: Node,
    _src: &[u8],
    stack: &mut [Scope],
    ctx: &mut WalkCtx,
) {
    let span = node_span(node, 0);
    for scope in stack.iter_mut().rev() {
        if let Some(b) = scope.vars.iter_mut().rev().find(|v| v.name == name) {
            if b.def_site.start_byte != span.start_byte {
                b.uses.push(span);
            }
            return;
        }
    }
    // Target not yet in scope — defer if we're in a Svelte context (the
    // <script> block may not have been walked yet).
    if ctx.is_svelte || ctx.is_ts_secondary {
        ctx.pending_refs.push(PendingRef {
            name: name.to_string(),
            span,
        });
    }
}

// --- Non-Svelte helpers (unchanged from original) ---

fn scope_kind_for(kind: &str) -> Option<ScopeKind> {
    match kind {
        "function_declaration"
        | "function_definition"
        | "function_item"
        | "method_declaration"
        | "method_definition"
        | "func_literal"
        | "arrow_function" => Some(ScopeKind::Function),
        "for_statement" | "while_statement" | "for_range_statement" | "for_in_clause" => {
            Some(ScopeKind::Loop)
        }
        _ => None,
    }
}

fn is_decl_node(kind: &str) -> bool {
    matches!(
        kind,
        // Go
        "short_var_declaration" | "var_spec" | "var_declaration" | "assignment"
        // TypeScript / JavaScript (secondary parse of Svelte <script>)
        | "lexical_declaration" | "variable_declaration"
    )
}

fn is_assign_node(kind: &str) -> bool {
    matches!(
        kind,
        // Go
        "assignment_statement" | "augmented_assignment"
        // TypeScript / JavaScript
        | "assignment_expression" | "augmented_assignment_expression"
    )
}

fn is_func_decl_node(kind: &str) -> bool {
    matches!(
        kind,
        // TypeScript / JavaScript named function declarations
        "function_declaration" | "generator_function_declaration"
    )
}

/// Bind a named function declaration's identifier to the PARENT scope
/// (the scope that was active BEFORE the Function scope was pushed).
/// This makes  visible as  at module/block level.
fn bind_func_name(node: Node, src: &[u8], stack: &mut [Scope], sid: &mut u32, offset: usize) {
    // The function name is the first  child of function_declaration.
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let child = cursor.node();
        if child.kind() == "identifier" {
            let name = text_of(child, src);
            *sid += 1;
            // Bind to the scope that encloses the function (second-to-last on stack,
            // since the Function scope was already pushed above us).
            let target_idx = if stack.len() >= 2 { stack.len() - 2 } else { 0 };
            stack[target_idx]
                .vars
                .push(new_binding(name, *sid, child, None, false, offset));
            return;
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

fn make_scope(kind: ScopeKind, node: Node, offset: usize) -> Scope {
    let sp = node_span(node, offset);
    Scope {
        kind,
        vars: Vec::new(),
        span: sp,
    }
}

fn node_span(node: Node, offset: usize) -> Span {
    Span {
        start_byte: node.start_byte() + offset,
        end_byte: node.end_byte() + offset,
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
    }
}

fn new_binding(
    name: String,
    sid: u32,
    node: Node,
    val: Option<ConstValue>,
    param: bool,
    offset: usize,
) -> VarBinding {
    VarBinding {
        name,
        sid,
        def_site: node_span(node, offset),
        def_value: val,
        taint_tags: SmallVec::new(),
        uses: Vec::new(),
        is_param: param,
    }
}

fn collect_params(
    node: Node,
    src: &[u8],
    query: &Query,
    stack: &mut [Scope],
    sid: &mut u32,
    offset: usize,
) {
    let Some(ni) = query.capture_index_for_name("name") else {
        return;
    };
    let mut qc = QueryCursor::new();
    let mut matches = qc.matches(query, node, src);
    while let Some(m) = matches.next() {
        for cap in m.captures.iter().filter(|c| c.index == ni) {
            *sid += 1;
            if let Some(scope) = stack.last_mut() {
                scope.vars.push(new_binding(
                    text_of(cap.node, src),
                    *sid,
                    cap.node,
                    None,
                    true,
                    offset,
                ));
            }
        }
    }
}

fn collect_bindings(
    node: Node,
    src: &[u8],
    query: &Query,
    stack: &mut [Scope],
    sid: &mut u32,
    param: bool,
    offset: usize,
) {
    let Some(ni) = query.capture_index_for_name("name") else {
        return;
    };
    let vi = query.capture_index_for_name("value");
    let mut qc = QueryCursor::new();
    let mut matches = qc.matches(query, node, src);
    while let Some(m) = matches.next() {
        let Some(nc) = m.captures.iter().find(|c| c.index == ni) else {
            continue;
        };
        let val = vi
            .and_then(|i| m.captures.iter().find(|c| c.index == i))
            .and_then(|vc| try_const(vc.node, src));
        *sid += 1;
        if let Some(scope) = stack.last_mut() {
            scope.vars.push(new_binding(
                text_of(nc.node, src),
                *sid,
                nc.node,
                val,
                param,
                offset,
            ));
        }
    }
}

fn record_reference(node: Node, src: &[u8], stack: &mut [Scope], ctx: &mut WalkCtx) {
    let name = text_of(node, src);
    let span = node_span(node, ctx.span_offset);
    for scope in stack.iter_mut().rev() {
        if let Some(b) = scope.vars.iter_mut().rev().find(|v| v.name == name) {
            if b.def_site.start_byte != span.start_byte {
                b.uses.push(span);
            }
            return;
        }
    }
    // Target not yet in scope — defer if we're in a Svelte context (the
    // <script> block may not have been walked yet).
    if ctx.is_svelte || ctx.is_ts_secondary {
        ctx.pending_refs.push(PendingRef { name, span });
    }
}

/// Resolve deferred references against the final scope stack (the still-open
/// enclosing scopes). Called after `walk_node` completes, before the stack is
/// drained into `finished`. Only enclosing scopes are searched, so a ref to an
/// already-popped function-local binding is NOT falsely resolved.
fn resolve_pending_refs(pending: &[PendingRef], stack: &mut [Scope]) {
    for pref in pending {
        for scope in stack.iter_mut().rev() {
            if let Some(b) = scope.vars.iter_mut().rev().find(|v| v.name == pref.name) {
                if b.def_site.start_byte != pref.span.start_byte {
                    b.uses.push(pref.span);
                }
                break;
            }
        }
    }
}

fn text_of(node: Node, src: &[u8]) -> String {
    std::str::from_utf8(&src[node.byte_range()])
        .unwrap_or("")
        .to_string()
}

fn try_const(node: Node, src: &[u8]) -> Option<ConstValue> {
    match node.kind() {
        "int_literal" | "integer" => text_of(node, src).parse::<i64>().ok().map(ConstValue::Int),
        "float_literal" | "float" => text_of(node, src)
            .parse::<f64>()
            .ok()
            .map(ConstValue::Float),
        "interpreted_string_literal" | "raw_string_literal" | "string" => {
            Some(ConstValue::Str(text_of(node, src)))
        }
        "true" | "false" | "True" | "False" => {
            Some(ConstValue::Bool(node.kind().starts_with(['t', 'T'])))
        }
        "nil" | "None" | "null" => Some(ConstValue::Nil),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
