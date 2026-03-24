//! Statement visitor helpers for [`IlBuilder`].
//!
//! Expression and l-value builders live in `il_build_lval.rs`.

use tree_sitter::Node;

use crate::il::*;
use crate::il_builder::IlBuilder;

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
