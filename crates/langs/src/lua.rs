use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_lua::LANGUAGE.into(),
        extensions: &["lua"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => {
            "(function_declaration) @scope \
             (function_definition) @scope"
        }
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => "(string) @scope",
        ScopeKind::TypeDefinitions => "(ERROR) @scope",
        ScopeKind::Imports => "(function_call) @scope",
    }
}
