use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_zig::LANGUAGE.into(),
        extensions: &["zig"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => "(function_declaration body: (block) @scope)",
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => "(string) @scope (multiline_string) @scope",
        ScopeKind::TypeDefinitions => {
            "(struct_declaration) @scope \
             (enum_declaration) @scope \
             (union_declaration) @scope"
        }
        ScopeKind::Imports => "(variable_declaration) @scope",
    }
}
