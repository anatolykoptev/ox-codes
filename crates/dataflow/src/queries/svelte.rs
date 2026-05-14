use tree_sitter::Language;

use super::LangQueries;

pub struct SvelteQueries {
    lang: Language,
}

impl Default for SvelteQueries {
    fn default() -> Self {
        Self { lang: tree_sitter_svelte_next::LANGUAGE.into() }
    }
}

impl SvelteQueries {
    pub fn new() -> Self { Self::default() }
}

impl LangQueries for SvelteQueries {
    // All real declarations live inside <script> raw_text which the scope
    // walker secondary-parses as TypeScript.  These stubs satisfy the trait
    // while producing zero matches at the Svelte-AST level.
    fn declarations_query(&self) -> &'static str { "" }
    fn assignments_query(&self) -> &'static str { "" }
    fn parameters_query(&self) -> &'static str { "" }
    fn references_query(&self) -> &'static str { "" }
    fn calls_query(&self) -> &'static str { "" }
    fn language(&self) -> &Language { &self.lang }
}
