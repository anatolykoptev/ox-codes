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

    let queries = CompiledQueries {
        decl: Query::new(&lang, lq.declarations_query())?,
        assign: Query::new(&lang, lq.assignments_query())?,
        param: Query::new(&lang, lq.parameters_query())?,
        _refs: Query::new(&lang, lq.references_query())?,
    };
    let mut ctx = WalkCtx { sid: 0, finished: Vec::new() };
    let mut stack = vec![make_scope(ScopeKind::Module, tree.root_node())];
    walk_node(tree.root_node(), source, &queries, &mut stack, &mut ctx);
    while let Some(s) = stack.pop() {
        ctx.finished.push(s);
    }
    Ok(ScopeChain { scopes: ctx.finished })
}

struct CompiledQueries { decl: Query, assign: Query, param: Query, _refs: Query }
struct WalkCtx { sid: u32, finished: Vec<Scope> }

fn walk_node(
    node: Node, src: &[u8], q: &CompiledQueries,
    stack: &mut Vec<Scope>, ctx: &mut WalkCtx,
) {
    let kind = node.kind();
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
    if push.is_some() {
        if let Some(s) = stack.pop() { ctx.finished.push(s); }
    }
}

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
        "short_var_declaration" | "var_spec" | "var_declaration" | "assignment"
    )
}

fn is_assign_node(kind: &str) -> bool {
    matches!(kind, "assignment_statement" | "augmented_assignment")
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
