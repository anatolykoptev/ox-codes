use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Deserialize)]
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

fn default_context_lines() -> usize { 2 }
fn default_max_results() -> usize { 50 }
fn default_true() -> bool { true }
