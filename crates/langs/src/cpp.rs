use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_cpp::LANGUAGE.into(),
        extensions: &["cpp", "cc", "cxx", "hpp", "hh"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => "(function_definition body: (compound_statement) @scope)",
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => {
            "(string_literal) @scope \
             (raw_string_literal) @scope"
        }
        ScopeKind::TypeDefinitions => {
            "(class_specifier) @scope \
             (struct_specifier) @scope \
             (enum_specifier) @scope"
        }
        ScopeKind::Imports => {
            "(preproc_include) @scope \
             (using_declaration) @scope"
        }
    }
}
