//! Statement visitor helpers for [`IlBuilder`].
//!
//! Expression and l-value builders live in `il_build_lval.rs`.

use tree_sitter::Node;

use crate::il::*;
use crate::il_builder::IlBuilder;
use crate::types::Span;

impl<'a> IlBuilder<'a> {
    pub(crate) fn visit_short_var_decl(&mut self, node: Node) {
        let lhs = node.child_by_field_name("left");
        let rhs = node.child_by_field_name("right");
        let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
            return;
        };
        // LHS is expression_list in Go; unwrap to first identifier.
        let lhs_ident = Self::unwrap_expr_list(lhs);
        let ident = self.node_text(lhs_ident).to_string();
        let sid = self.next_sid();
        self.name_table.insert(ident.clone(), sid);
        let lval = Lval::var(Name::new(ident, sid));
        let rval = self.build_expr(rhs);
        self.current_body.push(Instr::Assign {
            lval,
            rval,
            span: self.span_of(node),
        });
    }

    /// Handle TS/JS `const`/`let`/`var` declarations (`lexical_declaration` /
    /// `variable_declaration`).  Each `variable_declarator` child has `name`
    /// and optional `value` fields; the value (if present) is built as an
    /// expression and the result is pushed as an `Instr::Assign`.
    ///
    /// Destructuring declarators (`object_pattern` `const {a}=…` /
    /// `array_pattern` `const [a]=…`) are recursed: every LEAF identifier is
    /// bound to its own sid sourced from the SAME rval (#59). This
    /// over-approximates by design — every extracted binding inherits the full
    /// RHS taint (safe direction: no false negative; a false positive when
    /// only one property of the RHS is tainted is acceptable). The HARD LESSON
    /// from #55: NEVER use a pattern / non-identifier node's raw text as a
    /// binding name (a prior attempt coined a variable literally named
    /// "{token}" and poisoned the name table) — recurse DOWN to leaf
    /// `identifier` / `shorthand_property_identifier_pattern` nodes and bind
    /// ONLY those. Any pattern child kind not recognized here is CLEANLY
    /// SKIPPED (fail-safe: degrades to a false-negative skip, never a decoy
    /// binding or a panic).
    pub(crate) fn visit_var_decl(&mut self, node: Node) {
        let count = node.named_child_count();
        for i in 0..count {
            let Some(declarator) = node.named_child(i as u32) else {
                continue;
            };
            if declarator.kind() != "variable_declarator" {
                continue;
            }
            let name_node = declarator.child_by_field_name("name");
            let Some(name_node) = name_node else {
                continue;
            };
            let rval = declarator
                .child_by_field_name("value")
                .map(|n| self.build_expr(n))
                .unwrap_or(Expr::Const(Const::Nil));
            let span = self.span_of(node);
            // Branch on the declarator name shape. A plain identifier takes the
            // fast path; object/array patterns recurse to their leaf bindings.
            // Anything else is a clean skip (fail-safe).
            match name_node.kind() {
                "identifier" => self.bind_pattern_leaf(name_node, &rval, span),
                "object_pattern" | "array_pattern" => {
                    self.bind_pattern_leaves(name_node, &rval, span);
                }
                _ => {
                    // Unhandled declarator-name shape: skip cleanly (no decoy
                    // binding, no panic). Same fail-safe property the #55 guard
                    // gave, now reached per-declarator.
                }
            }
        }
    }

    /// Bind a single LEAF identifier pattern node to a fresh sid sourced from
    /// `rval` and push the `Instr::Assign`. `identifier` and
    /// `shorthand_property_identifier_pattern` are the only leaf kinds — both
    /// have node text that IS the binding name (e.g. `token` for `{token}`).
    fn bind_pattern_leaf(&mut self, leaf: Node, rval: &Expr, span: Span) {
        let ident = self.node_text(leaf).to_string();
        let sid = self.next_sid();
        self.name_table.insert(ident.clone(), sid);
        let lval = Lval::var(Name::new(ident, sid));
        self.current_body.push(Instr::Assign {
            lval,
            rval: rval.clone(),
            span,
        });
    }

    /// Recurse a destructuring pattern (`object_pattern` / `array_pattern` and
    /// their nested children), binding each leaf identifier to a fresh sid
    /// sourced from the SAME `rval`. Over-approximates: every extracted
    /// binding inherits the full RHS taint. Unrecognized child kinds are
    /// cleanly skipped (fail-safe).
    fn bind_pattern_leaves(&mut self, pattern: Node, rval: &Expr, span: Span) {
        match pattern.kind() {
            // Leaves: bind directly.
            "identifier" | "shorthand_property_identifier_pattern" => {
                self.bind_pattern_leaf(pattern, rval, span);
            }
            // Container patterns: recurse each named child.
            "object_pattern" | "array_pattern" => {
                let n = pattern.named_child_count() as u32;
                for i in 0..n {
                    if let Some(child) = pattern.named_child(i) {
                        self.bind_pattern_leaves(child, rval, span);
                    }
                }
            }
            // `{token: t}` — bind the VALUE side only; the property key
            // (`property_identifier`) is NOT a binding.
            "pair_pattern" => {
                let value = pattern.child_by_field_name("value").or_else(|| {
                    // Fallback: the named child that is not the key.
                    let n = pattern.named_child_count() as u32;
                    (0..n).find_map(|i| {
                        let c = pattern.named_child(i)?;
                        (c.kind() != "property_identifier").then_some(c)
                    })
                });
                if let Some(value) = value {
                    self.bind_pattern_leaves(value, rval, span);
                }
            }
            // `{...rest}` / `[...rest]` — bind the inner identifier.
            "rest_pattern" => {
                if let Some(inner) = pattern.named_child(0) {
                    self.bind_pattern_leaves(inner, rval, span);
                }
            }
            // `{token = 1}` (object_assignment_pattern) and `[a = 1]` /
            // `{a: b = 1}` (assignment_pattern): bind the LEFT (the identifier
            // being defaulted); the default value (right) is NOT a binding.
            "object_assignment_pattern" | "assignment_pattern" => {
                let left = pattern
                    .child_by_field_name("left")
                    .or_else(|| pattern.named_child(0));
                if let Some(left) = left {
                    self.bind_pattern_leaves(left, rval, span);
                }
            }
            // Unrecognized pattern child: clean skip (fail-safe — no decoy
            // binding, no panic). A future unhandled shape degrades to a
            // false-negative skip, never a decoy.
            _ => {}
        }
    }

    pub(crate) fn visit_assignment(&mut self, node: Node) {
        let lhs = node.child_by_field_name("left");
        let rhs = node.child_by_field_name("right");
        let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
            return;
        };
        let lval = self.build_lval(lhs);
        let rval = self.build_expr(rhs);
        self.current_body.push(Instr::Assign {
            lval,
            rval,
            span: self.span_of(node),
        });
    }

    pub(crate) fn visit_if(&mut self, node: Node) {
        if let Some(cond) = node.child_by_field_name("condition") {
            let cond_expr = self.build_expr(cond);
            self.current_body.push(Instr::Branch {
                cond: cond_expr,
                span: self.span_of(node),
            });
        }
        if let Some(body) = node.child_by_field_name("consequence") {
            self.visit_stmts(body);
        }
        if let Some(alt) = node.child_by_field_name("alternative") {
            self.visit_stmts(alt);
        }
    }

    pub(crate) fn visit_for(&mut self, node: Node) {
        // Visit all named children — handles Go (for_clause + body)
        // and Python (condition + body) uniformly.
        let count = node.child_count() as u32;
        for i in 0..count {
            if let Some(child) = node.child(i)
                && child.is_named()
            {
                self.visit_stmt(child);
            }
        }
    }

    pub(crate) fn visit_return(&mut self, node: Node) {
        let value = node
            .child_by_field_name("result")
            .or_else(|| {
                let count = node.child_count() as u32;
                (1..count).find_map(|i| {
                    let c = node.child(i)?;
                    if c.is_named() { Some(c) } else { None }
                })
            })
            .map(|n| self.build_expr(n));
        self.current_body.push(Instr::Return {
            value,
            span: self.span_of(node),
        });
    }

    pub(crate) fn visit_expr_stmt(&mut self, node: Node) {
        match node.kind() {
            "call_expression" | "call" => self.visit_call_stmt(node),
            "assignment" | "augmented_assignment" => self.visit_assignment(node),
            _ => {
                let _ = self.build_expr(node);
            }
        }
    }

    pub(crate) fn visit_call_stmt(&mut self, node: Node) {
        let func_node = node.child_by_field_name("function");
        let args_node = node.child_by_field_name("arguments");
        let func = func_node
            .map(|n| self.build_expr(n))
            .unwrap_or_else(|| Expr::Fixme(node.kind().into()));
        let args = args_node
            .map(|n| self.build_arg_list(n))
            .unwrap_or_default();
        self.current_body.push(Instr::Call {
            result: None,
            func,
            args,
            span: self.span_of(node),
        });
    }

    /// Handle Go's `for_clause` (init; cond; update) inside for_statement.
    pub(crate) fn visit_for_clause(&mut self, node: Node) {
        if let Some(init) = node.child_by_field_name("initializer") {
            self.visit_stmt(init);
        }
        if let Some(cond) = node.child_by_field_name("condition") {
            let cond_expr = self.build_expr(cond);
            self.current_body.push(Instr::Branch {
                cond: cond_expr,
                span: self.span_of(node),
            });
        }
        if let Some(update) = node.child_by_field_name("update") {
            self.visit_stmt(update);
        }
    }
}
