use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_java::LANGUAGE.into(),
        extensions: &["java"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => {
            "(method_declaration body: (block) @scope) \
             (constructor_declaration body: (constructor_body) @scope)"
        }
        ScopeKind::Comments => "(line_comment) @scope (block_comment) @scope",
        ScopeKind::Strings => "(string_literal) @scope",
        ScopeKind::TypeDefinitions => {
            "(class_declaration) @scope \
             (interface_declaration) @scope \
             (enum_declaration) @scope"
        }
        ScopeKind::Imports => "(import_declaration) @scope",
    }
}
