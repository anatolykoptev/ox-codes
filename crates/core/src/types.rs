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
    /// Hard cap on files walked. Defaults to
    /// [`crate::walk::DEFAULT_FILE_COUNT_CAP`] (2000). An explicit JSON `null`
    /// deserializes to `None` (walks everything) — the server clamps both
    /// `null` and oversized values to its transport max.
    #[serde(default = "default_max_files")]
    pub max_files: Option<usize>,
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
    /// Hard cap on files walked. Defaults to
    /// [`crate::walk::DEFAULT_FILE_COUNT_CAP`] (2000). An explicit JSON `null`
    /// deserializes to `None` (walks everything) — the server clamps both
    /// `null` and oversized values to its transport max.
    #[serde(default = "default_max_files")]
    pub max_files: Option<usize>,
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
    /// Hard cap on files walked. Defaults to
    /// [`crate::walk::DEFAULT_FILE_COUNT_CAP`] (2000). An explicit JSON `null`
    /// deserializes to `None` (walks everything) — the server clamps both
    /// `null` and oversized values to its transport max.
    #[serde(default = "default_max_files")]
    pub max_files: Option<usize>,
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
    /// Hard cap on files walked. Defaults to
    /// [`crate::walk::DEFAULT_FILE_COUNT_CAP`] (2000). An explicit JSON `null`
    /// deserializes to `None` (walks everything) — the server clamps both
    /// `null` and oversized values to its transport max.
    #[serde(default = "default_max_files")]
    pub max_files: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct RewriteResponse {
    pub files: Vec<RewriteFileResult>,
    pub total_matches: usize,
    /// Total edits dropped because their byte range overlapped/nested with an
    /// already-accepted edit (mirrors ast-grep CLI's conflicting-edit skip).
    /// `total_matches` counts only edits ACTUALLY applied.
    #[serde(skip_serializing_if = "is_zero")]
    pub total_skipped: usize,
    pub total_files: usize,
    pub duration_ms: u64,
    /// Files whose re-parse invariant failed and were NOT persisted. The batch
    /// continues past these (F2) so valid files still land; the bad ones are
    /// reported here instead of the whole call returning a 400.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<RewriteRejection>,
}

#[derive(Debug, Serialize)]
pub struct RewriteFileResult {
    pub file: String,
    pub matches: usize,
    /// Edits skipped for this file due to overlapping/nested match ranges.
    #[serde(skip_serializing_if = "is_zero")]
    pub skipped: usize,
    pub diff: String,
}

/// A file rejected from a `/rewrite apply=true` batch because its post-edit
/// re-parse invariant failed (new ERROR/MISSING nodes). The file is left
/// untouched on disk; the batch continues with the remaining files.
#[derive(Debug, Serialize)]
pub struct RewriteRejection {
    pub file: String,
    pub reason: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
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
    /// Total matches FOUND (pre-expansion, pre-`max_tokens`-budget). This is
    /// the raw per-file match count summed across all walked files — cheap
    /// and well-defined. Under `max_tokens=Some`, matches dropped during
    /// expansion (over-budget enclosing blocks) are still counted here.
    /// `truncated` is `total_matches > returned matches count`, so it stays
    /// truthful after backfill. For the default `max_tokens=None` path this
    /// equals the post-expansion count (no matches are dropped), so there is
    /// no drift on the default path.
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
fn default_max_files() -> Option<usize> {
    Some(crate::walk::DEFAULT_FILE_COUNT_CAP)
}
fn default_true() -> bool {
    true
}
