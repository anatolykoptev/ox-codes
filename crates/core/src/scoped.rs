use anyhow::Result;
use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use ignore::WalkBuilder;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tree_sitter::Query;

use crate::grep_filter::build_globset;
use crate::scope_cache::{CacheKey, CachedScopes, ScopeCache};
use crate::types::{ExpandMode, ExpandedMatch, ExpandedSearchResponse, ScopedSearchInput};
use ox_langs::{get_language, get_scope_query};

pub fn scoped_search(
    input: ScopedSearchInput,
    cache: &ScopeCache,
) -> Result<ExpandedSearchResponse> {
    let start = Instant::now();

    let scope = parse_scope_kind(&input.scope)?;

    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!input.case_sensitive)
        .build(&input.pattern)?;

    let lang_cfg = get_language(&input.language)
        .ok_or_else(|| anyhow::anyhow!("unsupported language: {}", input.language))?;
    let query_str = get_scope_query(&input.language, scope)
        .ok_or_else(|| anyhow::anyhow!("no scope query for {}/{:?}", input.language, scope))?;

    let query = Arc::new(Query::new(&lang_cfg.language, query_str)?);
    let language = lang_cfg.language;

    let include = input.file_glob.as_deref().map(build_globset).transpose()?;
    let exclude = input
        .exclude_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    let mut all_matches: Vec<ExpandedMatch> = Vec::new();
    let root = Path::new(&input.root);

    for entry in WalkBuilder::new(root)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
    {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !lang_cfg.extensions.contains(&ext) {
            continue;
        }

        let rel_path = path.strip_prefix(root).unwrap_or(path);
        if let Some(ref inc) = include
            && !inc.is_match(rel_path)
        {
            continue;
        }
        if let Some(ref exc) = exclude
            && exc.is_match(rel_path)
        {
            continue;
        }

        let canonical = match std::fs::canonicalize(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let metadata = match std::fs::metadata(&canonical) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime_nanos = metadata
            .modified()
            .unwrap_or(std::time::UNIX_EPOCH)
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let file_len = metadata.len();

        let key = CacheKey {
            canonical_abs_path: canonical.clone(),
            mtime_nanos,
            file_len,
            language: input.language.clone(),
            scope_kind: scope,
        };

        let query_ref = Arc::clone(&query);
        let lang = language.clone();
        let cached = cache.get_or_insert(key, move || {
            let source = std::fs::read(&canonical)?;
            let scopes = ScopeCache::parse_scopes(source, &query_ref, &lang)?;
            Ok(Arc::new(scopes))
        })?;

        let file_matches = search_in_scopes(
            &cached,
            &language,
            &matcher,
            &rel_path.to_string_lossy(),
            &input.expand,
            input.max_tokens,
            input.format,
            &input.language,
        );
        all_matches.extend(file_matches);

        if all_matches.len() >= input.max_results * 5 {
            break;
        }
    }

    let total = all_matches.len();
    let truncated = total > input.max_results;
    if truncated {
        all_matches.truncate(input.max_results);
    }

    Ok(ExpandedSearchResponse {
        matches: all_matches,
        total_matches: total,
        truncated,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

#[allow(clippy::too_many_arguments)]
fn search_in_scopes(
    cached: &CachedScopes,
    language: &tree_sitter::Language,
    matcher: &grep_regex::RegexMatcher,
    rel_path: &str,
    expand: &ExpandMode,
    max_tokens: Option<usize>,
    format: crate::types::Format,
    lang: &str,
) -> Vec<ExpandedMatch> {
    let mut matches = Vec::new();

    for span in &cached.spans {
        let scope_text = &cached.source[span.start..span.end];
        let scope_str = String::from_utf8_lossy(scope_text);
        let mut cumulative_bytes: usize = 0;
        for (line_offset, line) in scope_str.lines().enumerate() {
            if matcher.is_match(line.as_bytes()).unwrap_or(false) {
                let match_byte = span.start + cumulative_bytes;

                let expanded = if !matches!(expand, ExpandMode::None) {
                    let block = crate::expand::find_enclosing_symbol(
                        &cached.source,
                        language,
                        match_byte,
                        expand,
                    );
                    if let (Some(b), Some(max_tok)) = (&block, max_tokens) {
                        let estimated_tokens = b.body.len() / 4;
                        if estimated_tokens > max_tok {
                            cumulative_bytes += line.len() + 1;
                            continue;
                        }
                    }
                    block.map(|blk| crate::types::ExpandedBlock {
                        body: crate::expand::wrap_body(blk.body, format, Some(lang)),
                        ..blk
                    })
                } else {
                    None
                };

                matches.push(ExpandedMatch {
                    file: rel_path.to_string(),
                    line: span.start_line + line_offset,
                    text: line.trim_end().to_string(),
                    context: vec![],
                    expanded,
                });
            }
            cumulative_bytes += line.len() + 1;
        }
    }

    matches
}

fn parse_scope_kind(s: &str) -> Result<ox_langs::ScopeKind> {
    match s {
        "function_bodies" | "functions" => Ok(ox_langs::ScopeKind::FunctionBodies),
        "comments" => Ok(ox_langs::ScopeKind::Comments),
        "strings" => Ok(ox_langs::ScopeKind::Strings),
        "type_definitions" | "types" => Ok(ox_langs::ScopeKind::TypeDefinitions),
        "imports" => Ok(ox_langs::ScopeKind::Imports),
        _ => anyhow::bail!(
            "unknown scope kind: {s}. Valid: function_bodies, comments, strings, type_definitions, imports"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExpandMode;
    use std::fs;
    use std::time::Duration;
    use tempfile::TempDir;

    fn setup_go_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            r#"package main

// TODO: this is a comment TODO
import "fmt"

func main() {
    // TODO inside function
    fmt.Println("hello TODO")
}

type Config struct {
    Name string // TODO in type
}
"#,
        )
        .unwrap();
        dir
    }

    fn setup_go_repo_three() -> TempDir {
        let dir = TempDir::new().unwrap();
        let files = [
            ("a.go", "package main\n\nfunc A() {\n    // TODO in A\n}\n"),
            ("b.go", "package main\n\nfunc B() {\n    // TODO in B\n}\n"),
            ("c.go", "package main\n\nfunc C() {\n    // TODO in C\n}\n"),
        ];
        for (name, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
        }
        dir
    }

    fn go_input(root: &str) -> ScopedSearchInput {
        ScopedSearchInput {
            root: root.into(),
            pattern: "TODO".into(),
            scope: "function_bodies".into(),
            language: "go".into(),
            is_regex: false,
            max_results: 50,
            case_sensitive: true,
            file_glob: None,
            exclude_glob: None,
            expand: ExpandMode::default(),
            max_tokens: None,
            format: crate::types::Format::Plain,
        }
    }

    #[test]
    fn test_scoped_function_bodies() {
        let dir = setup_go_repo();
        let input = go_input(&dir.path().to_string_lossy());
        let result = scoped_search(input, &ScopeCache::new()).unwrap();
        assert!(!result.matches.is_empty());
        assert!(result.matches.iter().all(|m| m.line >= 6));
    }

    #[test]
    fn test_scoped_comments() {
        let dir = setup_go_repo();
        let input = ScopedSearchInput {
            scope: "comments".into(),
            ..go_input(&dir.path().to_string_lossy())
        };
        let result = scoped_search(input, &ScopeCache::new()).unwrap();
        assert!(result.matches.len() >= 2);
    }

    #[test]
    fn test_scoped_strings() {
        let dir = setup_go_repo();
        let input = ScopedSearchInput {
            scope: "strings".into(),
            ..go_input(&dir.path().to_string_lossy())
        };
        let result = scoped_search(input, &ScopeCache::new()).unwrap();
        assert_eq!(result.matches.len(), 1);
    }

    #[test]
    fn test_invalid_scope() {
        let dir = setup_go_repo();
        let input = ScopedSearchInput {
            scope: "invalid_scope".into(),
            ..go_input(&dir.path().to_string_lossy())
        };
        assert!(scoped_search(input, &ScopeCache::new()).is_err());
    }

    #[test]
    fn test_scoped_with_expand() {
        let dir = setup_go_repo();
        let input = ScopedSearchInput {
            scope: "comments".into(),
            expand: ExpandMode::Function,
            ..go_input(&dir.path().to_string_lossy())
        };
        let result = scoped_search(input, &ScopeCache::new()).unwrap();
        let expanded_matches: Vec<_> = result
            .matches
            .iter()
            .filter(|m| m.expanded.is_some())
            .collect();
        assert!(!expanded_matches.is_empty());
        let block = expanded_matches[0].expanded.as_ref().unwrap();
        assert_eq!(block.symbol_name, "main");
    }

    #[test]
    fn test_cache_hit_contract() {
        let dir = setup_go_repo_three();
        let cache = ScopeCache::with_capacity(64 * 1024 * 1024);
        let input = go_input(&dir.path().to_string_lossy());

        let run1 = scoped_search(input.clone(), &cache).unwrap();
        let (h1, m1) = cache.stats();
        assert_eq!(m1, 3, "first run should parse all three files");
        assert_eq!(h1, 0, "first run should have no cache hits");
        assert!(run1.matches.len() >= 3);

        let run2 = scoped_search(input, &cache).unwrap();
        let (h2, m2) = cache.stats();
        assert_eq!(m2, 3, "second run should have no additional misses");
        assert_eq!(h2, 3, "second run should hit all three files");
        assert_eq!(run1.matches, run2.matches);
    }

    #[test]
    fn test_cache_invalidation() {
        let dir = setup_go_repo_three();
        let cache = ScopeCache::with_capacity(64 * 1024 * 1024);
        let input = go_input(&dir.path().to_string_lossy());

        let _ = scoped_search(input.clone(), &cache).unwrap();
        let (_, m1) = cache.stats();
        assert_eq!(m1, 3);

        // Append a byte to one file, changing mtime + len.
        let a_path = dir.path().join("a.go");
        let mut content = fs::read_to_string(&a_path).unwrap();
        content.push('\n');
        fs::write(&a_path, content).unwrap();
        // Give filesystem a moment to update mtime.
        std::thread::sleep(Duration::from_millis(10));

        let _ = scoped_search(input, &cache).unwrap();
        let (h2, m2) = cache.stats();
        assert_eq!(m2, 4, "modified file should be a miss");
        assert_eq!(h2, 2, "unchanged files should be hits");
    }

    #[test]
    fn test_cache_concurrency_smoke() {
        let dir = setup_go_repo_three();
        let cache = ScopeCache::with_capacity(64 * 1024 * 1024);
        let root = dir.path().to_string_lossy().into_owned();

        let results: Vec<_> = std::thread::scope(|s| {
            let cache_ref = &cache;
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let root = root.clone();
                    s.spawn(move || {
                        let input = go_input(&root);
                        scoped_search(input, cache_ref)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap().unwrap())
                .collect()
        });

        let first = &results[0];
        for result in results.iter().skip(1) {
            assert_eq!(result.matches, first.matches);
            assert_eq!(result.total_matches, first.total_matches);
        }

        let (_, misses) = cache.stats();
        assert!(misses >= 3, "each file should be parsed at least once");
    }
}
