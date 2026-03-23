use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_ruby::LANGUAGE.into(),
        extensions: &["rb"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => "(method body: (body_statement) @scope)",
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => "(string) @scope",
        ScopeKind::TypeDefinitions => {
            "(class body: (body_statement) @scope) \
             (module body: (body_statement) @scope)"
        }
        ScopeKind::Imports => "(call) @scope",
    }
}
