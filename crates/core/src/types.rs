use serde::{Deserialize, Serialize};

/// Controls how matches are expanded to surrounding AST context.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpandMode {
    /// No expansion — return matched line only (default).
    #[default]
    None,
    /// Expand to enclosing function/method.
    Function,
    /// Expand to enclosing block (function, struct, class, impl, etc.).
    Block,
}

#[derive(Debug, Deserialize)]
pub struct SearchInput {
    pub root: String,
    pub pattern: String,
    #[serde(default)]
    pub is_regex: bool,
    #[serde(default)]
    pub file_glob: Option<String>,
    #[serde(default)]
    pub exclude_glob: Option<String>,
    #[serde(default = "default_context_lines")]
    pub context_lines: usize,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub expand: ExpandMode,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub format: Format,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScopedSearchInput {
    pub root: String,
    pub pattern: String,
    pub scope: String,
    pub language: String,
    #[serde(default)]
    pub is_regex: bool,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
    #[serde(default)]
    pub file_glob: Option<String>,
    #[serde(default)]
    pub exclude_glob: Option<String>,
    #[serde(default)]
    pub expand: ExpandMode,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub format: Format,
}

#[derive(Debug, Deserialize)]
pub struct StructuralSearchInput {
    pub root: String,
    pub pattern: String,
    pub language: String,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default)]
    pub file_glob: Option<String>,
    #[serde(default)]
    pub exclude_glob: Option<String>,
    #[serde(default)]
    pub expand: ExpandMode,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub format: Format,
}

#[derive(Debug, Deserialize)]
pub struct RewriteInput {
    pub root: String,
    pub pattern: String,
    pub rewrite: String,
    pub language: String,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default)]
    pub file_glob: Option<String>,
    #[serde(default)]
    pub exclude_glob: Option<String>,
    #[serde(default)]
    pub apply: bool,
}

#[derive(Debug, Serialize)]
pub struct RewriteResponse {
    pub files: Vec<RewriteFileResult>,
    pub total_matches: usize,
    pub total_files: usize,
    pub duration_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct RewriteFileResult {
    pub file: String,
    pub matches: usize,
    pub diff: String,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub matches: Vec<SearchMatch>,
    pub total_matches: usize,
    pub truncated: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchMatch {
    pub file: String,
    pub line: usize,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
}

/// A match expanded to its surrounding AST context.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExpandedMatch {
    pub file: String,
    pub line: usize,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    /// Full text of the enclosing AST node (when expand != None).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expanded: Option<ExpandedBlock>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExpandedBlock {
    pub symbol_name: String,
    pub symbol_kind: String,
    pub line_start: usize,
    pub line_end: usize,
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct ExpandedSearchResponse {
    pub matches: Vec<ExpandedMatch>,
    pub total_matches: usize,
    pub truncated: bool,
    pub duration_ms: u64,
}

/// Output format for expanded bodies.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Plain,
    Markdown,
}

fn default_context_lines() -> usize {
    2
}
fn default_max_results() -> usize {
    50
}
fn default_true() -> bool {
    true
}
