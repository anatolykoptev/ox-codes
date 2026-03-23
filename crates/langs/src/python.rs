use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_python::LANGUAGE.into(),
        extensions: &["py"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => "(function_definition body: (block) @scope)",
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => "(string) @scope",
        ScopeKind::TypeDefinitions => "(class_definition) @scope",
        ScopeKind::Imports => {
            "(import_statement) @scope \
             (import_from_statement) @scope"
        }
    }
}
