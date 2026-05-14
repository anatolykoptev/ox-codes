use crate::{LangConfig, ScopeKind};

/// Svelte files are parsed with the tree-sitter-svelte grammar.
/// Template expressions are secondary-parsed as TypeScript by the scope walker.
pub fn config() -> LangConfig {
    LangConfig {
        language: tree_sitter_svelte_next::LANGUAGE.into(),
        extensions: &["svelte"],
    }
}

pub fn scope_query(scope: ScopeKind) -> &'static str {
    // Svelte grammar has no JS-level declarations at the top level;
    // all declarations live inside `script_element > raw_text` (parsed
    // as TypeScript by the scope walker's secondary parse).
    // Queries here are intentionally minimal stubs.
    match scope {
        ScopeKind::FunctionBodies => "",
        ScopeKind::Comments => "(comment) @scope",
        ScopeKind::Strings => "(quoted_attribute_value) @scope",
        ScopeKind::TypeDefinitions => "",
        ScopeKind::Imports => "",
    }
}
