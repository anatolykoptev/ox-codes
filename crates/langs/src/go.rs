use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_go::LANGUAGE.into(),
        extensions: &["go"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => {
            "(function_declaration body: (block) @scope) \
             (method_declaration body: (block) @scope)"
        }
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => {
            "(interpreted_string_literal) @scope \
             (raw_string_literal) @scope"
        }
        ScopeKind::TypeDefinitions => "(type_declaration) @scope",
        ScopeKind::Imports => "(import_declaration) @scope",
    }
}
