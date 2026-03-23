use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_php::LANGUAGE_PHP.into(),
        extensions: &["php"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => {
            "(function_definition body: (compound_statement) @scope) \
             (method_declaration body: (compound_statement) @scope)"
        }
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => "(string) @scope (encapsed_string) @scope",
        ScopeKind::TypeDefinitions => {
            "(class_declaration) @scope \
             (interface_declaration) @scope \
             (trait_declaration) @scope"
        }
        ScopeKind::Imports => "(namespace_use_declaration) @scope",
    }
}
