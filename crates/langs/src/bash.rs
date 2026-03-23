use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_bash::LANGUAGE.into(),
        extensions: &["sh", "bash"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => {
            "(function_definition body: (compound_statement) @scope)"
        }
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => "(string) @scope (raw_string) @scope",
        ScopeKind::TypeDefinitions => "(ERROR) @scope",
        ScopeKind::Imports => "(command) @scope",
    }
}
