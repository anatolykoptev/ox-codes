use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_c_sharp::LANGUAGE.into(),
        extensions: &["cs"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => {
            "(method_declaration body: (block) @scope) \
             (constructor_declaration body: (block) @scope)"
        }
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => {
            "(string_literal) @scope \
             (interpolated_string_expression) @scope"
        }
        ScopeKind::TypeDefinitions => {
            "(class_declaration) @scope \
             (interface_declaration) @scope \
             (enum_declaration) @scope \
             (struct_declaration) @scope"
        }
        ScopeKind::Imports => "(using_directive) @scope",
    }
}
