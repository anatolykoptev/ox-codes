use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_c::LANGUAGE.into(),
        extensions: &["c", "h"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => "(function_definition body: (compound_statement) @scope)",
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => "(string_literal) @scope",
        ScopeKind::TypeDefinitions => {
            "(struct_specifier) @scope \
             (enum_specifier) @scope \
             (type_definition) @scope"
        }
        ScopeKind::Imports => "(preproc_include) @scope",
    }
}
