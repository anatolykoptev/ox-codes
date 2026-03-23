use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_rust::LANGUAGE.into(),
        extensions: &["rs"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => {
            "(function_item body: (block) @scope) \
             (impl_item body: (declaration_list) @scope)"
        }
        ScopeKind::Comments => "(line_comment) @scope (block_comment) @scope",
        ScopeKind::Strings => {
            "(string_literal) @scope \
             (raw_string_literal) @scope"
        }
        ScopeKind::TypeDefinitions => {
            "(struct_item) @scope \
             (enum_item) @scope \
             (type_item) @scope"
        }
        ScopeKind::Imports => "(use_declaration) @scope",
    }
}
