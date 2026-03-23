use globset::{Glob, GlobSet, GlobSetBuilder};
use anyhow::Result;

/// Map a language name to its file extensions.
pub fn lang_extensions(lang: &str) -> Option<&'static [&'static str]> {
    match lang.to_lowercase().as_str() {
        "go" => Some(&["go"]),
        "rust" | "rs" => Some(&["rs"]),
        "python" | "py" => Some(&["py"]),
        "typescript" | "ts" => Some(&["ts", "tsx"]),
        "javascript" | "js" => Some(&["js", "jsx"]),
        "java" => Some(&["java"]),
        "c" => Some(&["c", "h"]),
        "cpp" | "c++" => Some(&["cpp", "cc", "cxx", "hpp", "hh"]),
        "ruby" | "rb" => Some(&["rb"]),
        "php" => Some(&["php"]),
        "swift" => Some(&["swift"]),
        "kotlin" | "kt" => Some(&["kt", "kts"]),
        _ => None,
    }
}

/// Build a GlobSet from a comma-separated list of patterns.
/// Patterns without `/` are automatically prefixed with `**/` to match recursively
/// (e.g. `*_test.go` becomes `**/*_test.go` to match `internal/foo_test.go`).
pub fn build_globset(patterns: &str) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pat in patterns.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let effective = if !pat.contains('/') {
            format!("**/{pat}")
        } else {
            pat.to_string()
        };
        builder.add(Glob::new(&effective)?);
    }
    Ok(builder.build()?)
}

/// Check whether a relative path matches a language filter.
pub fn matches_language(rel_path: &str, exts: &[&str]) -> bool {
    let lower = rel_path.to_lowercase();
    exts.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
}
