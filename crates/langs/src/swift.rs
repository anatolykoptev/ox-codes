use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_swift::LANGUAGE.into(),
        extensions: &["swift"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => {
            "(function_declaration body: (function_body) @scope)"
        }
        ScopeKind::Comments => "(comment) @scope (multiline_comment) @scope",
        ScopeKind::Strings => {
            "(line_string_literal) @scope \
             (multi_line_string_literal) @scope"
        }
        ScopeKind::TypeDefinitions => {
            "(class_declaration) @scope \
             (protocol_declaration) @scope"
        }
        ScopeKind::Imports => "(import_declaration) @scope",
    }
}
