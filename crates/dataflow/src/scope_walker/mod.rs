use anyhow::{Context, Result};
use smallvec::SmallVec;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::queries;
use crate::types::{ConstValue, Scope, ScopeChain, ScopeKind, Span, VarBinding};

/// Walk a source file and build a scope chain with variable bindings.
pub fn walk_file(source: &[u8], lang_name: &str) -> Result<ScopeChain> {
    let lq = queries::get_queries(lang_name)
        .with_context(|| format!("unsupported language: {lang_name}"))?;
    let lang = lq.language().clone();
    let mut parser = Parser::new();
    parser.set_language(&lang)?;
    let tree = parser.parse(source, None).context("parse failed")?;

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

    let mut ctx = WalkCtx { sid: 0, finished: Vec::new(), is_svelte, is_ts_secondary: false };
    let mut stack = vec![make_scope(ScopeKind::Module, tree.root_node())];
    walk_node(tree.root_node(), source, &compiled, &mut stack, &mut ctx);
    while let Some(s) = stack.pop() {
        ctx.finished.push(s);
    }
    Ok(ScopeChain { scopes: ctx.finished })
}

/// Build CompiledQueries for TypeScript (used for Svelte secondary parses).
fn build_ts_queries(ts_lang: &tree_sitter::Language) -> Result<CompiledQueries> {
    use crate::queries::typescript::TypescriptQueries;
    use crate::queries::LangQueries;
    let tq = TypescriptQueries::new();
    Ok(CompiledQueries {
        decl: Query::new(ts_lang, tq.declarations_query())?,
        assign: Query::new(ts_lang, tq.assignments_query())?,
        param: Query::new(ts_lang, tq.parameters_query())?,
    })
}

/// Secondary-parse a UTF-8 slice as TypeScript and walk it, recording
/// references and bindings into the current scope stack.
fn walk_as_typescript(
    raw: &[u8],
    q: &CompiledQueries,
    stack: &mut Vec<Scope>,
    ctx: &mut WalkCtx,
) {
    let ts_lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let mut parser = Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return;
    }
    let Some(tree) = parser.parse(raw, None) else { return };
    // Walk the secondary tree; mark as non-svelte so we use compiled TS queries.
    let saved_svelte = ctx.is_svelte;
    ctx.is_svelte = false;
    ctx.is_ts_secondary = true;
    walk_node(tree.root_node(), raw, q, stack, ctx);
    ctx.is_svelte = saved_svelte;
    ctx.is_ts_secondary = false;
}

struct CompiledQueries { decl: Query, assign: Query, param: Query }
struct WalkCtx { sid: u32, finished: Vec<Scope>, is_svelte: bool, is_ts_secondary: bool }

fn walk_node(
    node: Node, src: &[u8], q: &CompiledQueries,
    stack: &mut Vec<Scope>, ctx: &mut WalkCtx,
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
            stack.push(make_scope(sk, node));
            if sk == ScopeKind::Function {
                collect_params(node, src, &q.param, stack, &mut ctx.sid);
            }
        }
        if is_decl_node(kind) {
            collect_bindings(node, src, &q.decl, stack, &mut ctx.sid, false);
        } else if is_assign_node(kind) {
            collect_bindings(node, src, &q.assign, stack, &mut ctx.sid, false);
        } else if ctx.is_ts_secondary && is_func_decl_node(kind) {
            // Bind the function name to the enclosing scope (not the function's own scope).
            bind_func_name(node, src, stack, &mut ctx.sid);
        } else if node.child_count() == 0 && kind == "identifier" {
            record_reference(node, src, stack);
        }
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                walk_node(cursor.node(), src, q, stack, ctx);
                if !cursor.goto_next_sibling() { break; }
            }
        }
        if push.is_some()
            && let Some(s) = stack.pop() {
                ctx.finished.push(s);
            }
        return;
    }

    // For Svelte nodes not handled by early return above: recurse into children.
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_node(cursor.node(), src, q, stack, ctx);
            if !cursor.goto_next_sibling() { break; }
        }
    }
}

// --- Svelte helper functions ---

/// Walk `<script>` element: find `raw_text` child and secondary-parse as TS.
fn walk_svelte_script(
    node: Node, src: &[u8], q: &CompiledQueries,
    stack: &mut Vec<Scope>, ctx: &mut WalkCtx,
) {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() { return; }
    loop {
        let child = cursor.node();
        if child.kind() == "raw_text" {
            let raw = &src[child.byte_range()];
            walk_as_typescript(raw, q, stack, ctx);
        }
        if !cursor.goto_next_sibling() { break; }
    }
}

/// Walk `expression` node: extract `svelte_raw_text` child and secondary-parse.
fn walk_svelte_expression(
    node: Node, src: &[u8], q: &CompiledQueries,
    stack: &mut Vec<Scope>, ctx: &mut WalkCtx,
) {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() { return; }
    loop {
        let child = cursor.node();
        if child.kind() == "svelte_raw_text" {
            let raw = &src[child.byte_range()];
            walk_as_typescript(raw, q, stack, ctx);
        }
        if !cursor.goto_next_sibling() { break; }
    }
}

/// Walk `attribute` node: if attribute_name is a directive, secondary-parse
/// the expression child; otherwise walk normally.
fn walk_svelte_attribute(
    node: Node, src: &[u8], q: &CompiledQueries,
    stack: &mut Vec<Scope>, ctx: &mut WalkCtx,
) {
    use ox_langs::preproc::svelte_refs;

    // Find attribute_name and expression children
    let mut attr_name: Option<String> = None;
    let mut expressions: Vec<Node> = Vec::new();
    let mut cursor = node.walk();
    if !cursor.goto_first_child() { return; }
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
        if !cursor.goto_next_sibling() { break; }
    }

    // If it's a directive that carries an expression, secondary-parse each.
    if let Some(name) = &attr_name
        && let Some(directive) = svelte_refs::parse_directive(name) {
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
                        record_name_as_reference(&directive.name, node, src, stack);
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
    node: Node, field: &str, src: &[u8], q: &CompiledQueries,
    stack: &mut Vec<Scope>, ctx: &mut WalkCtx,
) {
    if let Some(cond) = node.child_by_field_name(field)
        && cond.kind() == "svelte_raw_text" {
            let raw = &src[cond.byte_range()];
            walk_as_typescript(raw, q, stack, ctx);
        }
}

/// Walk `each_start` node: process the iterable expression (identifier field)
/// and record the binding (parameter field) as a declared variable.
fn walk_svelte_each_start(
    node: Node, src: &[u8], q: &CompiledQueries,
    stack: &mut Vec<Scope>, ctx: &mut WalkCtx,
) {
    // identifier field: the expression being iterated (e.g. `items`)
    if let Some(id_node) = node.child_by_field_name("identifier")
        && id_node.kind() == "svelte_raw_text" {
            let raw = &src[id_node.byte_range()];
            walk_as_typescript(raw, q, stack, ctx);
        }
    // parameter field: the binding name (e.g. `item`) — declare, don't reference
    if let Some(param_node) = node.child_by_field_name("parameter")
        && param_node.kind() == "svelte_raw_text" {
            let name = text_of(param_node, src).trim().to_string();
            if !name.is_empty() {
                ctx.sid += 1;
                if let Some(scope) = stack.last_mut() {
                    scope.vars.push(new_binding(name, ctx.sid, param_node, None, true));
                }
            }
        }
}

/// Walk all `svelte_raw_text` children of a node as TypeScript expressions.
fn walk_svelte_raw_text_children(
    node: Node, src: &[u8], q: &CompiledQueries,
    stack: &mut Vec<Scope>, ctx: &mut WalkCtx,
) {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() { return; }
    loop {
        let child = cursor.node();
        if child.kind() == "svelte_raw_text" {
            let raw = &src[child.byte_range()];
            walk_as_typescript(raw, q, stack, ctx);
        }
        if !cursor.goto_next_sibling() { break; }
    }
}

/// Synthesize a zero-span reference from a plain name string.
fn record_name_as_reference(name: &str, node: Node, _src: &[u8], stack: &mut [Scope]) {
    let span = node_span(node);
    for scope in stack.iter_mut().rev() {
        if let Some(b) = scope.vars.iter_mut().rev().find(|v| v.name == name) {
            if b.def_site.start_byte != span.start_byte {
                b.uses.push(span);
            }
            return;
        }
    }
}

// --- Non-Svelte helpers (unchanged from original) ---

fn scope_kind_for(kind: &str) -> Option<ScopeKind> {
    match kind {
        "function_declaration" | "function_definition" | "function_item"
        | "method_declaration" | "method_definition" | "func_literal"
        | "arrow_function" => Some(ScopeKind::Function),
        "for_statement" | "while_statement" | "for_range_statement"
        | "for_in_clause" => Some(ScopeKind::Loop),
        _ => None,
    }
}

fn is_decl_node(kind: &str) -> bool {
    matches!(kind,
        // Go
        "short_var_declaration" | "var_spec" | "var_declaration" | "assignment"
        // TypeScript / JavaScript (secondary parse of Svelte <script>)
        | "lexical_declaration" | "variable_declaration"
    )
}

fn is_assign_node(kind: &str) -> bool {
    matches!(kind,
        // Go
        "assignment_statement" | "augmented_assignment"
        // TypeScript / JavaScript
        | "assignment_expression" | "augmented_assignment_expression"
    )
}

fn is_func_decl_node(kind: &str) -> bool {
    matches!(kind,
        // TypeScript / JavaScript named function declarations
        "function_declaration" | "generator_function_declaration"
    )
}

/// Bind a named function declaration's identifier to the PARENT scope
/// (the scope that was active BEFORE the Function scope was pushed).
/// This makes  visible as  at module/block level.
fn bind_func_name(node: Node, src: &[u8], stack: &mut [Scope], sid: &mut u32) {
    // The function name is the first  child of function_declaration.
    let mut cursor = node.walk();
    if !cursor.goto_first_child() { return; }
    loop {
        let child = cursor.node();
        if child.kind() == "identifier" {
            let name = text_of(child, src);
            *sid += 1;
            // Bind to the scope that encloses the function (second-to-last on stack,
            // since the Function scope was already pushed above us).
            let target_idx = if stack.len() >= 2 { stack.len() - 2 } else { 0 };
            stack[target_idx].vars.push(new_binding(name, *sid, child, None, false));
            return;
        }
        if !cursor.goto_next_sibling() { break; }
    }
}

fn make_scope(kind: ScopeKind, node: Node) -> Scope {
    let sp = node_span(node);
    Scope { kind, vars: Vec::new(), span: sp }
}

fn node_span(node: Node) -> Span {
    Span {
        start_byte: node.start_byte(), end_byte: node.end_byte(),
        start_line: node.start_position().row + 1, end_line: node.end_position().row + 1,
    }
}

fn new_binding(name: String, sid: u32, node: Node, val: Option<ConstValue>, param: bool) -> VarBinding {
    VarBinding {
        name, sid, def_site: node_span(node), def_value: val,
        taint_tags: SmallVec::new(), uses: Vec::new(), is_param: param,
    }
}

fn collect_params(
    node: Node, src: &[u8], query: &Query, stack: &mut [Scope], sid: &mut u32,
) {
    let Some(ni) = query.capture_index_for_name("name") else { return };
    let mut qc = QueryCursor::new();
    let mut matches = qc.matches(query, node, src);
    while let Some(m) = matches.next() {
        for cap in m.captures.iter().filter(|c| c.index == ni) {
            *sid += 1;
            if let Some(scope) = stack.last_mut() {
                scope.vars.push(new_binding(text_of(cap.node, src), *sid, cap.node, None, true));
            }
        }
    }
}

fn collect_bindings(
    node: Node, src: &[u8], query: &Query,
    stack: &mut [Scope], sid: &mut u32, param: bool,
) {
    let Some(ni) = query.capture_index_for_name("name") else { return };
    let vi = query.capture_index_for_name("value");
    let mut qc = QueryCursor::new();
    let mut matches = qc.matches(query, node, src);
    while let Some(m) = matches.next() {
        let Some(nc) = m.captures.iter().find(|c| c.index == ni) else { continue };
        let val = vi.and_then(|i| m.captures.iter().find(|c| c.index == i))
            .and_then(|vc| try_const(vc.node, src));
        *sid += 1;
        if let Some(scope) = stack.last_mut() {
            scope.vars.push(new_binding(text_of(nc.node, src), *sid, nc.node, val, param));
        }
    }
}

fn record_reference(node: Node, src: &[u8], stack: &mut [Scope]) {
    let name = text_of(node, src);
    let span = node_span(node);
    for scope in stack.iter_mut().rev() {
        if let Some(b) = scope.vars.iter_mut().rev().find(|v| v.name == name) {
            if b.def_site.start_byte != span.start_byte {
                b.uses.push(span);
            }
            return;
        }
    }
}

fn text_of(node: Node, src: &[u8]) -> String {
    std::str::from_utf8(&src[node.byte_range()]).unwrap_or("").to_string()
}

fn try_const(node: Node, src: &[u8]) -> Option<ConstValue> {
    match node.kind() {
        "int_literal" | "integer" => text_of(node, src).parse::<i64>().ok().map(ConstValue::Int),
        "float_literal" | "float" => text_of(node, src).parse::<f64>().ok().map(ConstValue::Float),
        "interpreted_string_literal" | "raw_string_literal" | "string" =>
            Some(ConstValue::Str(text_of(node, src))),
        "true" | "false" | "True" | "False" =>
            Some(ConstValue::Bool(node.kind().starts_with(['t', 'T']))),
        "nil" | "None" | "null" => Some(ConstValue::Nil),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
