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
    //
    // IMPORTANT: tree-sitter-typescript has NO visibility into Svelte-specific
    // syntax. The following constructs are invisible to these queries and will
    // be silently ignored:
    //   - Template expressions: {#if}, {#each}, {#await}, {:else}, {/if}, etc.
    //   - Event directives:     on:click={handler}, on:input={...}
    //   - Reactive statements:  $: derived = expr  (Svelte's $: label)
    //   - Slot props:           let:item, <slot let:foo={bar}>
    //
    // To add proper Svelte grammar support, replace tree-sitter-typescript with
    // the tree-sitter-svelte grammar (https://github.com/Himujjal/tree-sitter-svelte)
    // and extend the queries below to cover the above node types.
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
