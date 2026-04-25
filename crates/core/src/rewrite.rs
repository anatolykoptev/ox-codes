use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Result};
use ast_grep_core::matcher::Pattern;
use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_core::AstGrep;
use ignore::WalkBuilder;

use crate::grep_filter::build_globset;
use crate::structural::{file_matches_lang, lang_wrapper};
use crate::types::{RewriteFileResult, RewriteInput, RewriteResponse};

pub fn rewrite(input: RewriteInput) -> Result<RewriteResponse> {
    let start = Instant::now();

    let lang_name = input.language.to_lowercase();
    let wrapper = match lang_wrapper(&lang_name) {
        Some(w) => w,
        None => bail!("unsupported language: {}", input.language),
    };

    let pattern = Pattern::try_new(&input.pattern, wrapper.clone())
        .map_err(|e| anyhow::anyhow!("invalid pattern '{}': {e}", input.pattern))?;

    let include = input.file_glob.as_deref().map(build_globset).transpose()?;
    let exclude = input.exclude_glob.as_deref().map(build_globset).transpose()?;

    let root = Path::new(&input.root);
    let mut files: Vec<RewriteFileResult> = Vec::new();
    let mut total_matches = 0usize;

    let walker = WalkBuilder::new(root)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()));

    for entry in walker {
        let path = entry.path();
        if !file_matches_lang(path, &lang_name) {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(path);
        if let Some(ref inc) = include
            && !inc.is_match(rel)
        {
            continue;
        }
        if let Some(ref exc) = exclude
            && exc.is_match(rel)
        {
            continue;
        }

        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let rel_path = rel.to_string_lossy().into_owned();
        let ast: AstGrep<StrDoc<_>> = AstGrep::new(&src, wrapper.clone());

        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        for node_match in ast.root().find_all(pattern.clone()) {
            let edit = node_match.replace_by(input.rewrite.as_str());
            let replacement = String::from_utf8_lossy(&edit.inserted_text).into_owned();
            edits.push((edit.position, edit.deleted_length, replacement));
        }

        if edits.is_empty() {
            continue;
        }

        let match_count = edits.len();
        total_matches += match_count;

        let modified = apply_edits(&src, edits);

        if input.apply {
            // Atomic write: NamedTempFile gives unique name + persist() does rename(2).
            // WalkBuilder(follow_links=false) means we only write files inside root.
            let dir = path.parent().ok_or_else(|| anyhow::anyhow!("no parent for {}", path.display()))?;
            let mut tmp = tempfile::NamedTempFile::new_in(dir)
                .map_err(|e| anyhow::anyhow!("rewrite: create tmp in {}: {e}", dir.display()))?;
            use std::io::Write as _;
            tmp.write_all(modified.as_bytes())
                .map_err(|e| anyhow::anyhow!("rewrite: write tmp: {e}"))?;
            tmp.persist(path)
                .map_err(|e| anyhow::anyhow!("rewrite: persist {}: {e}", path.display()))?;
        }

        let diff = unified_diff(&rel_path, &src, &modified);

        files.push(RewriteFileResult {
            file: rel_path,
            matches: match_count,
            diff,
        });

        if total_matches >= input.max_results {
            break;
        }
    }

    let total_files = files.len();
    Ok(RewriteResponse {
        files,
        total_matches,
        total_files,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Apply a list of (position, deleted_length, replacement) edits to source.
fn apply_edits(source: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|(pos, _, _)| *pos);
    let mut result = source.to_string();
    for (pos, del_len, replacement) in edits.into_iter().rev() {
        let end = (pos + del_len).min(result.len());
        result.replace_range(pos..end, &replacement);
    }
    result
}

fn unified_diff(file_path: &str, original: &str, modified: &str) -> String {
    similar::TextDiff::from_lines(original, modified)
        .unified_diff()
        .header(&format!("a/{file_path}"), &format!("b/{file_path}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_rewrite_go_error_wrapping() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            "package main\n\nfunc foo() error {\n    val, err := doSomething()\n    if err != nil {\n        return err\n    }\n    return nil\n}\n",
        ).unwrap();
        let input = RewriteInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "if $ERR != nil { return $ERR }".into(),
            rewrite: "if $ERR != nil { return fmt.Errorf(\"wrap: %w\", $ERR) }".into(),
            language: "go".into(),
            max_results: 50,
            file_glob: None,
            exclude_glob: None,
            apply: false,
        };
        let result = rewrite(input).unwrap();
        assert_eq!(result.total_matches, 1);
        assert_eq!(result.total_files, 1);
        assert!(!result.files[0].diff.is_empty());
        assert!(result.files[0].diff.contains("fmt.Errorf"));
    }

    #[test]
    fn test_rewrite_multiple_matches_in_file() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            "package main\n\nfunc foo() error {\n    if err != nil {\n        return err\n    }\n    if err2 != nil {\n        return err2\n    }\n    return nil\n}\n",
        ).unwrap();
        let input = RewriteInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "if $E != nil { return $E }".into(),
            rewrite: "if $E != nil { return fmt.Errorf(\"%w\", $E) }".into(),
            language: "go".into(),
            max_results: 50,
            file_glob: None,
            exclude_glob: None,
            apply: false,
        };
        let result = rewrite(input).unwrap();
        assert_eq!(result.total_matches, 2);
        assert_eq!(result.total_files, 1);
    }

    #[test]
    fn test_rewrite_no_match() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.go"), "package main\n\nfunc main() {}\n").unwrap();
        let input = RewriteInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "if $X != nil { return $X }".into(),
            rewrite: "replaced".into(),
            language: "go".into(),
            max_results: 50,
            file_glob: None,
            exclude_glob: None,
            apply: false,
        };
        let result = rewrite(input).unwrap();
        assert_eq!(result.total_matches, 0);
        assert!(result.files.is_empty());
    }

    #[test]
    fn test_apply_edits_reverse_order() {
        let src = "aaa bbb ccc";
        let edits = vec![
            (0, 3, "YYY".to_string()),
            (4, 3, "XXX".to_string()),
        ];
        let result = apply_edits(src, edits);
        assert_eq!(result, "YYY XXX ccc");
    }

    #[test]
    fn test_unified_diff_format() {
        let original = "line1\nline2\nline3\n";
        let modified = "line1\nchanged\nline3\n";
        let diff = unified_diff("test.go", original, modified);
        assert!(diff.contains("--- a/test.go"));
        assert!(diff.contains("+++ b/test.go"));
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+changed"));
    }

    #[test]
    fn test_rewrite_apply_writes_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("main.go");
        fs::write(&file_path, "package main\nfunc f() {\nif err != nil { return err }\n}\n").unwrap();
        let input = RewriteInput {
            root: dir.path().to_str().unwrap().to_string(),
            pattern: "if $ERR != nil { return $ERR }".into(),
            rewrite: "if $ERR != nil { return fmt.Errorf(\"wrap: %w\", $ERR) }".into(),
            language: "go".into(),
            max_results: 10,
            file_glob: None,
            exclude_glob: None,
            apply: true,
        };
        let result = rewrite(input).unwrap();
        assert_eq!(result.total_matches, 1);
        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("fmt.Errorf"), "file not updated on disk: {}", content);
    }
}
