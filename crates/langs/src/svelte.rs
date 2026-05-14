use crate::{LangConfig, ScopeKind};

/// Svelte files are parsed with the TypeScript grammar (covers `<script>` blocks).
pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        extensions: &["svelte"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    // Delegate to TypeScript queries — tree-sitter-typescript parses
    // the <script> block content inside .svelte files correctly.
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
