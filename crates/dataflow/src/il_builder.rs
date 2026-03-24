//! Translates tree-sitter ASTs into the three-address IL.
//! Uses direct `node.kind()` dispatch — no tree-sitter queries needed.

use std::collections::HashMap;

use anyhow::Result;
use tree_sitter::{Node, Parser};

use crate::il::*;
use crate::types::{Sid, Span};

/// Build IL from source code for a given language.
pub fn build_il(source: &[u8], lang_name: &str) -> Result<IlFile> {
    let lang_cfg = ox_langs::get_language(lang_name)
        .ok_or_else(|| anyhow::anyhow!("unsupported language: {lang_name}"))?;
    let mut parser = Parser::new();
    parser.set_language(&lang_cfg.language)?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("parse failed"))?;

    let mut builder = IlBuilder::new(source);
    builder.visit(tree.root_node());
    Ok(builder.finish())
}

pub(crate) struct IlBuilder<'a> {
    pub(crate) source: &'a [u8],
    pub(crate) functions: Vec<IlFunction>,
    pub(crate) sid_counter: Sid,
    pub(crate) current_body: Vec<Instr>,
    pub(crate) current_params: Vec<Name>,
    pub(crate) current_name: String,
    pub(crate) current_span: Option<Span>,
    pub(crate) name_table: HashMap<String, Sid>,
}

impl<'a> IlBuilder<'a> {
    pub(crate) fn new(source: &'a [u8]) -> Self {
        Self {
            source,
            functions: Vec::new(),
            sid_counter: 0,
            current_body: Vec::new(),
            current_params: Vec::new(),
            current_name: String::new(),
            current_span: None,
            name_table: HashMap::new(),
        }
    }

    pub(crate) fn finish(self) -> IlFile {
        IlFile {
            functions: self.functions,
        }
    }

    pub(crate) fn next_sid(&mut self) -> Sid {
        self.sid_counter += 1;
        self.sid_counter
    }

    pub(crate) fn node_text(&self, node: Node) -> &str {
        node.utf8_text(self.source).unwrap_or("")
    }

    pub(crate) fn span_of(&self, node: Node) -> Span {
        let start = node.start_position();
        let end = node.end_position();
        Span {
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            start_line: start.row + 1,
            end_line: end.row + 1,
        }
    }

    fn visit(&mut self, node: Node) {
        match node.kind() {
            "function_declaration" | "method_declaration" | "function_definition" => {
                self.visit_function(node);
            }
            _ => {
                let count = node.child_count() as u32;
                for i in 0..count {
                    if let Some(child) = node.child(i) {
                        self.visit(child);
                    }
                }
            }
        }
    }

    fn visit_function(&mut self, node: Node) {
        let saved_body = std::mem::take(&mut self.current_body);
        let saved_params = std::mem::take(&mut self.current_params);
        let saved_name = std::mem::take(&mut self.current_name);
        let saved_span = self.current_span.take();
        let saved_names = std::mem::take(&mut self.name_table);

        self.current_span = Some(self.span_of(node));
        self.current_name = self.extract_func_name(node);
        self.extract_params(node);
        self.visit_body(node);

        let func = IlFunction {
            name: std::mem::take(&mut self.current_name),
            params: std::mem::take(&mut self.current_params),
            body: std::mem::take(&mut self.current_body),
            span: self.current_span.take().unwrap(),
        };
        self.functions.push(func);

        self.current_body = saved_body;
        self.current_params = saved_params;
        self.current_name = saved_name;
        self.current_span = saved_span;
        self.name_table = saved_names;
    }

    fn extract_func_name(&self, node: Node) -> String {
        node.child_by_field_name("name")
            .map(|n| self.node_text(n).to_string())
            .unwrap_or_else(|| "<anon>".into())
    }

    fn extract_params(&mut self, node: Node) {
        let params_node = node
            .child_by_field_name("parameters")
            .or_else(|| node.child_by_field_name("params"));
        let Some(params) = params_node else { return };
        let count = params.child_count() as u32;
        for i in 0..count {
            let Some(child) = params.child(i) else {
                continue;
            };
            let name_text = match child.kind() {
                "parameter_declaration" | "typed_parameter" | "typed_default_parameter" => child
                    .child_by_field_name("name")
                    .map(|n| self.node_text(n).to_string()),
                "identifier" => Some(self.node_text(child).to_string()),
                _ => None,
            };
            if let Some(ident) = name_text {
                let sid = self.next_sid();
                self.name_table.insert(ident.clone(), sid);
                self.current_params.push(Name::new(ident, sid));
            }
        }
    }

    fn visit_body(&mut self, func_node: Node) {
        let body = func_node
            .child_by_field_name("body")
            .or_else(|| func_node.child_by_field_name("block"));
        let Some(body) = body else { return };
        self.visit_stmts(body);
    }

    pub(crate) fn visit_stmts(&mut self, node: Node) {
        let count = node.child_count() as u32;
        for i in 0..count {
            if let Some(child) = node.child(i) {
                self.visit_stmt(child);
            }
        }
    }

    pub(crate) fn visit_stmt(&mut self, node: Node) {
        match node.kind() {
            "short_var_declaration" => self.visit_short_var_decl(node),
            "assignment_statement" | "assignment" | "augmented_assignment" => {
                self.visit_assignment(node);
            }
            "if_statement" => self.visit_if(node),
            "for_statement" | "while_statement" => self.visit_for(node),
            "return_statement" => self.visit_return(node),
            "expression_statement" => {
                if let Some(expr) = node.named_child(0) {
                    self.visit_expr_stmt(expr);
                }
            }
            "call" | "call_expression" => self.visit_call_stmt(node),
            "go_statement" | "defer_statement" | "select_statement"
            | "try_statement" | "with_statement" => {
                self.current_body.push(Instr::Fixme {
                    reason: node.kind().to_string(),
                    span: self.span_of(node),
                });
            }
            "block" | "block_statement" | "statement_list" => {
                self.visit_stmts(node);
            }
            "for_clause" => self.visit_for_clause(node),
            "inc_statement" | "dec_statement" => { /* skip */ }
            _ => { /* skip punctuation, comments, etc. */ }
        }
    }
}
