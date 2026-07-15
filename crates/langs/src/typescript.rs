use crate::{LangConfig, ScopeKind};

pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        extensions: &["ts", "tsx", "js", "jsx"],
    }
}

/// Config for the JSX-aware TSX grammar (`.tsx`/`.jsx` files).
pub fn config_tsx() -> LangConfig {
    LangConfig {
        language: tree_sitter_typescript::LANGUAGE_TSX.into(),
        extensions: &["tsx", "jsx"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::FunctionBodies => {
            "(function_declaration body: (statement_block) @scope) \
             (arrow_function body: (statement_block) @scope) \
             (method_definition body: (statement_block) @scope)"
        }
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => "(string) @scope (template_string) @scope",
        ScopeKind::TypeDefinitions => {
            "(interface_declaration) @scope \
             (type_alias_declaration) @scope \
             (class_declaration) @scope"
        }
        ScopeKind::Imports => "(import_statement) @scope",
    }
}
