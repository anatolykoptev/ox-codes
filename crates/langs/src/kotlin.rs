use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_kotlin_ng::LANGUAGE.into(),
        extensions: &["kt", "kts"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => "(function_declaration) @scope",
        ScopeKind::Comments => "(line_comment) @scope (block_comment) @scope",
        ScopeKind::Strings => {
            "(string_literal) @scope \
             (multiline_string_literal) @scope"
        }
        ScopeKind::TypeDefinitions => {
            "(class_declaration) @scope \
             (object_declaration) @scope"
        }
        ScopeKind::Imports => "(import) @scope",
    }
}
