use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};

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
    for pat in split_top_level_commas(patterns)
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let effective = if !pat.contains('/') {
            format!("**/{pat}")
        } else {
            pat.to_string()
        };
        builder.add(Glob::new(&effective)?);
    }
    Ok(builder.build()?)
}

/// Split `patterns` on top-level commas only — a `,` inside a `{...}` brace
/// alternation group belongs to the group, not a glob separator. This preserves
/// standard brace globs like `**/*.{ts,js}` while keeping the comma-separated
/// multi-glob contract (`a,b` → two globs). Brace depth is tracked with a simple
/// counter; an unbalanced `{` (no closing `}`) leaves the rest of the string in
/// one fragment, which then fails at `Glob::new` with the original parse error.
fn split_top_level_commas(patterns: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0;
    for (i, ch) in patterns.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ',' if depth == 0 => {
                out.push(&patterns[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&patterns[start..]);
    out
}

/// Check whether a relative path matches a language filter.
pub fn matches_language(rel_path: &str, exts: &[&str]) -> bool {
    let lower = rel_path.to_lowercase();
    exts.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(gs: &GlobSet, path: &str) -> bool {
        gs.is_match(path)
    }

    #[test]
    fn brace_alternation_glob_preserved() {
        // **/*.{ts,js} must stay ONE glob — comma inside {} is NOT a separator.
        let gs = build_globset("**/*.{ts,js}").expect("brace glob should build");
        assert!(matches(&gs, "a.ts"), "should match a.ts");
        assert!(matches(&gs, "a.js"), "should match a.js");
        assert!(!matches(&gs, "a.rs"), "should not match a.rs");
    }

    #[test]
    fn comma_separated_multi_glob_still_works() {
        // back-compat: comma at top level splits into two globs.
        let gs = build_globset("**/*.ts,**/*.js").expect("multi-glob should build");
        assert!(matches(&gs, "a.ts"), "should match a.ts");
        assert!(matches(&gs, "a.js"), "should match a.js");
        assert!(!matches(&gs, "a.rs"), "should not match a.rs");
    }

    #[test]
    fn mixed_brace_and_comma_splits_at_top_level_only() {
        // src/**/*.{ts,tsx},lib/*.js -> two globs; brace group intact in first.
        let gs = build_globset("src/**/*.{ts,tsx},lib/*.js").expect("mixed should build");
        assert!(matches(&gs, "src/a.ts"), "should match src/a.ts");
        assert!(matches(&gs, "src/a.tsx"), "should match src/a.tsx");
        assert!(matches(&gs, "lib/a.js"), "should match lib/a.js");
        assert!(!matches(&gs, "src/a.js"), "should not match src/a.js");
        assert!(!matches(&gs, "lib/a.ts"), "should not match lib/a.ts");
    }

    #[test]
    fn genuinely_unclosed_brace_still_errors() {
        // **/*.{ts — no closing } — must remain a parse error.
        let err = build_globset("**/*.{ts").unwrap_err();
        assert!(
            err.to_string().contains("unclosed") || err.to_string().contains("alternate"),
            "expected unclosed/alternate error, got: {err}"
        );
    }
}
