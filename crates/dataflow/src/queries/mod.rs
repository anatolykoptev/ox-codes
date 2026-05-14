use tree_sitter::Language;

pub mod go;
pub mod python;
pub mod rust_lang;
pub mod typescript;
#[cfg(test)]
mod test_util;

/// Per-language query configuration for data-flow analysis.
///
/// Each language provides tree-sitter queries to identify:
/// - Variable declarations (let, var, :=)
/// - Assignments (=, +=, etc.)
/// - Function parameters
/// - Identifier references (reads)
/// - Call expressions (for taint sinks)
pub trait LangQueries {
    /// Query to capture variable declarations.
    /// Capture names: @name (variable identifier), @value (initial value, optional).
    fn declarations_query(&self) -> &'static str;

    /// Query to capture assignments.
    /// Capture names: @name (left-hand side), @value (right-hand side).
    fn assignments_query(&self) -> &'static str;

    /// Query to capture function parameters.
    /// Capture names: @name (parameter identifier).
    fn parameters_query(&self) -> &'static str;

    /// Query to capture identifier references (reads).
    /// Capture names: @name (the identifier).
    fn references_query(&self) -> &'static str;

    /// Query to capture call expressions.
    /// Capture names: @func (function being called), @args (argument list).
    fn calls_query(&self) -> &'static str;

    /// Get the tree-sitter Language for query compilation.
    fn language(&self) -> &Language;
}

/// Get queries for a language by name.
pub fn get_queries(name: &str) -> Option<Box<dyn LangQueries>> {
    match name {
        "go" | "golang" => Some(Box::new(go::GoQueries::new())),
        "python" | "py" => Some(Box::new(python::PythonQueries::new())),
        "typescript" | "ts" | "javascript" | "js" | "svelte" => Some(Box::new(typescript::TypescriptQueries::new())),
        "rust" | "rs" => Some(Box::new(rust_lang::RustQueries::new())),
        _ => None,
    }
}
