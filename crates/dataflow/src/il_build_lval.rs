//! Expression and l-value builders for [`IlBuilder`].
//!
//! Split from `il_build_expr.rs` to stay under the 200-line limit.

use tree_sitter::Node;

use crate::il::*;
use crate::il_builder::IlBuilder;

impl<'a> IlBuilder<'a> {
    pub(crate) fn build_expr(&mut self, node: Node) -> Expr {
        match node.kind() {
            "identifier" | "field_identifier" => {
                let text = self.node_text(node).to_string();
                let sid = self.resolve_or_create(&text);
                Expr::Lval(Lval::var(Name::new(text, sid)))
            }
            "int_literal" | "integer" | "number" => {
                let text = self.node_text(node);
                let val = text.parse::<i64>().unwrap_or(0);
                Expr::Const(Const::Int(val))
            }
            "float_literal" | "float" => {
                let text = self.node_text(node);
                let val = text.parse::<f64>().unwrap_or(0.0);
                Expr::Const(Const::Float(val))
            }
            "interpreted_string_literal" | "raw_string_literal"
            | "string" | "string_literal" => {
                Expr::Const(Const::Str(self.node_text(node).to_string()))
            }
            "true" | "True" => Expr::Const(Const::Bool(true)),
            "false" | "False" => Expr::Const(Const::Bool(false)),
            "nil" | "None" => Expr::Const(Const::Nil),
            "binary_expression" | "boolean_operator" | "comparison_operator" => {
                self.build_binop(node)
            }
            "unary_expression" | "not_operator" => self.build_unary(node),
            "call_expression" | "call" => self.build_call_expr(node),
            "parenthesized_expression" => node
                .child(1)
                .map(|c| self.build_expr(c))
                .unwrap_or_else(|| Expr::Fixme("empty_parens".into())),
            "selector_expression" | "attribute" => Expr::Lval(self.build_lval(node)),
            "expression_list" => node
                .named_child(0)
                .map(|c| self.build_expr(c))
                .unwrap_or_else(|| Expr::Fixme("empty_expr_list".into())),
            _ => Expr::Fixme(node.kind().to_string()),
        }
    }

    fn build_binop(&mut self, node: Node) -> Expr {
        let left = node
            .child_by_field_name("left")
            .map(|n| self.build_expr(n))
            .unwrap_or_else(|| Expr::Fixme("missing_left".into()));
        let right = node
            .child_by_field_name("right")
            .map(|n| self.build_expr(n))
            .unwrap_or_else(|| Expr::Fixme("missing_right".into()));
        let op = node
            .child_by_field_name("operator")
            .map(|n| self.node_text(n).to_string())
            .unwrap_or_else(|| "?".into());
        Expr::BinOp {
            op,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    fn build_unary(&mut self, node: Node) -> Expr {
        let operand = node
            .child_by_field_name("operand")
            .or_else(|| node.child_by_field_name("argument"))
            .map(|n| self.build_expr(n))
            .unwrap_or_else(|| Expr::Fixme("missing_operand".into()));
        let op = node
            .child_by_field_name("operator")
            .map(|n| self.node_text(n).to_string())
            .unwrap_or_else(|| "?".into());
        Expr::UnaryOp {
            op,
            operand: Box::new(operand),
        }
    }

    fn build_call_expr(&mut self, node: Node) -> Expr {
        let func = node
            .child_by_field_name("function")
            .map(|n| self.build_expr(n))
            .unwrap_or_else(|| Expr::Fixme("missing_func".into()));
        let args = node
            .child_by_field_name("arguments")
            .map(|n| self.build_arg_list(n))
            .unwrap_or_default();
        Expr::Call {
            func: Box::new(func),
            args,
        }
    }

    pub(crate) fn build_arg_list(&mut self, node: Node) -> Vec<Expr> {
        let mut args = Vec::new();
        let count = node.child_count() as u32;
        for i in 0..count {
            if let Some(child) = node.child(i)
                && child.is_named()
            {
                args.push(self.build_expr(child));
            }
        }
        args
    }

    pub(crate) fn build_lval(&mut self, node: Node) -> Lval {
        match node.kind() {
            "identifier" => {
                let text = self.node_text(node).to_string();
                let sid = self.resolve_or_create(&text);
                Lval::var(Name::new(text, sid))
            }
            "selector_expression" | "attribute" => {
                let base_node = node
                    .child_by_field_name("operand")
                    .or_else(|| node.child_by_field_name("object"));
                let field_node = node
                    .child_by_field_name("field")
                    .or_else(|| node.child_by_field_name("attribute"));
                let mut lval = base_node
                    .map(|n| self.build_lval(n))
                    .unwrap_or_else(|| Lval {
                        base: Base::Unknown(self.node_text(node).into()),
                        offsets: Default::default(),
                    });
                if let Some(f) = field_node {
                    lval.offsets
                        .push(Offset::Field(self.node_text(f).to_string()));
                }
                lval
            }
            "expression_list" => {
                let inner = Self::unwrap_expr_list(node);
                self.build_lval(inner)
            }
            _ => Lval {
                base: Base::Unknown(self.node_text(node).to_string()),
                offsets: Default::default(),
            },
        }
    }

    /// Unwrap expression_list to its first named child, or return as-is.
    pub(crate) fn unwrap_expr_list(node: Node) -> Node {
        if node.kind() == "expression_list" {
            node.named_child(0).unwrap_or(node)
        } else {
            node
        }
    }

    /// Resolve an identifier to its sid, creating a new one if unseen.
    fn resolve_or_create(&mut self, name: &str) -> u32 {
        if let Some(&sid) = self.name_table.get(name) {
            sid
        } else {
            let sid = self.next_sid();
            self.name_table.insert(name.to_string(), sid);
            sid
        }
    }
}
