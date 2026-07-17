use anyhow::Result;
use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tree_sitter::Query;

use crate::grep_filter::build_globset;
use crate::scope_cache::{CacheKey, CachedScopes, ScopeCache, hash_content};
use crate::types::{ExpandMode, ExpandedMatch, ExpandedSearchResponse, ScopedSearchInput};
use crate::walk::{DEFAULT_MAX_FILE_BYTES, WalkBudget, filtered_walk};
use ox_langs::{effective_language_id, get_language, get_scope_query, language_id};

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
    let canonical_id = language_id(&input.language)
        .ok_or_else(|| anyhow::anyhow!("unsupported language: {}", input.language))?;
    let query_str = get_scope_query(canonical_id, scope)
        .ok_or_else(|| anyhow::anyhow!("no scope query for {}/{:?}", canonical_id, scope))?;

    let include = input.file_glob.as_deref().map(build_globset).transpose()?;
    let exclude = input
        .exclude_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    let mut all_matches: Vec<ExpandedMatch> = Vec::new();
    let root = Path::new(&input.root);

    let budget = WalkBudget {
        max_files: input.max_files,
        // #52: files >20MB are skipped pre-read by `ignore`'s `max_filesize`.
        max_file_bytes: Some(DEFAULT_MAX_FILE_BYTES),
    };
    for (path, rel, _metadata) in filtered_walk(
        root,
        Some(lang_cfg.extensions),
        include.as_ref(),
        exclude.as_ref(),
        budget,
    ) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let canonical = match std::fs::canonicalize(&path) {
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
            language: canonical_id.to_string(),
            scope_kind: scope,
        };

        // Resolve the grammar PER-FILE from the walked file's extension.
        // For .tsx/.jsx under a TypeScript-family language, the JSX-aware
        // TSX grammar must be used instead of the non-JSX LANGUAGE_TYPESCRIPT,
        // which produces ERROR nodes on JSX and can cause the parser to fail
        // to recognize subsequent function declarations (dropping their scope
        // spans entirely).  Grammar selection goes through
        // ox_langs::effective_language_id + get_language (single source of truth).
        let per_file_lang = effective_language_id(&input.language, ext)
            .and_then(|id| get_language(id).map(|c| c.language))
            .unwrap_or_else(|| lang_cfg.language.clone());
        let lang = per_file_lang.clone();
        let verify_path = canonical.clone();
        let (cached, _is_hit) = cache.get_or_insert_verified(
            key,
            move || {
                // Compile the scope query lazily INSIDE the closure so it runs only
                // on a cache MISS — building it per-file before this point
                // recompiled the tree-sitter Query even on cache HITs, defeating
                // the point of ScopeCache (re-review #55).
                let query = Query::new(&lang, query_str)?;
                let source = std::fs::read(&canonical)?;
                let scopes = ScopeCache::parse_scopes(source, &query, &lang)?;
                Ok(Arc::new(scopes))
            },
            |cached| {
                // #48: on a (mtime, len) key match, re-read the file and
                // compare a content hash to the one stored at insert time. A
                // mismatch means a same-length in-place edit landed within
                // the filesystem's mtime resolution — the entry is stale.
                match std::fs::read(&verify_path) {
                    Ok(bytes) => hash_content(&bytes) == cached.content_hash,
                    // File vanished/unreadable between the stat above and this re-read
                    // (TOCTOU). Keep the cached entry (stale-but-successful) instead of
                    // returning false: a false here forces a re-init whose own read would
                    // also fail and hard-error the whole request. This matches the walk's
                    // resilient-skip on missing files (see the `continue`s at metadata/
                    // canonicalize above) and the pre-#48 hit-path behaviour.
                    Err(_) => true,
                }
            },
        )?;

        let file_matches = search_in_scopes(
            &cached,
            &per_file_lang,
            &matcher,
            &rel,
            &input.expand,
            input.max_tokens,
            input.format,
            canonical_id,
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
            max_files: Some(crate::walk::DEFAULT_FILE_COUNT_CAP),
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
    fn test_cache_alias_key() {
        let dir = setup_go_repo_three();
        let cache = ScopeCache::with_capacity(64 * 1024 * 1024);
        let root = dir.path().to_string_lossy().into_owned();
        let input = go_input(&root);

        let _ = scoped_search(input.clone(), &cache).unwrap();
        let (_, m1) = cache.stats();
        assert_eq!(
            m1, 3,
            "first run with canonical 'go' should parse all files"
        );

        let alias_input = ScopedSearchInput {
            language: "golang".into(),
            ..input
        };
        let _ = scoped_search(alias_input, &cache).unwrap();
        let (h2, m2) = cache.stats();
        assert_eq!(m2, 3, "alias 'golang' should not create additional misses");
        assert_eq!(
            h2, 3,
            "alias 'golang' should hit the canonical 'go' entries"
        );
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

    /// Regression test for issue #44 in the /search/scoped route.
    ///
    /// A `.tsx` file with JSX BEFORE a function declaration.  Under the
    /// non-JSX `LANGUAGE_TYPESCRIPT` grammar (the old fixed-grammar walk),
    /// the JSX `const a = <Foo bar={x}>baz</Foo>` produces `ERROR` nodes
    /// that break parser recovery — the subsequent `function App()` is not
    /// recognized, so the `function_bodies` scope query produces no spans
    /// for it, and the pattern `secret` inside the function body is never
    /// searched.  With per-file grammar selection (TSX grammar for `.tsx`),
    /// the JSX is parsed correctly, the function is recognized, and the
    /// pattern is found.
    #[test]
    fn test_scoped_tsx_function_not_dropped() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("App.tsx"),
            r#"const a = <Foo bar={x}>baz</Foo>;
function App() {
    const secret = getSecret();
    return <div>{secret}</div>;
}
"#,
        )
        .unwrap();

        let input = ScopedSearchInput {
            root: dir.path().to_string_lossy().into_owned(),
            pattern: "secret".into(),
            scope: "function_bodies".into(),
            language: "typescript".into(),
            is_regex: false,
            max_results: 50,
            case_sensitive: true,
            file_glob: None,
            exclude_glob: None,
            expand: ExpandMode::None,
            max_tokens: None,
            format: crate::types::Format::Plain,
            max_files: Some(crate::walk::DEFAULT_FILE_COUNT_CAP),
        };
        let result = scoped_search(input, &ScopeCache::new()).unwrap();
        assert!(
            !result.matches.is_empty(),
            "secret must be found inside the .tsx function body; \
             got {} matches (grammar selection may be wrong)",
            result.matches.len()
        );
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

    // ── PR2: filtered_walk adoption tests ───────────────────────────────

    /// Golden / result-identity test: adopting `filtered_walk` + `WalkBudget`
    /// did NOT change what `/search/scoped` returns for a normal request over
    /// normal-sized files. The match count and line numbers must be exactly
    /// what they were before the refactor.
    #[test]
    fn test_scoped_golden_result_identity() {
        let dir = setup_go_repo();
        let input = go_input(&dir.path().to_string_lossy());
        let result = scoped_search(input, &ScopeCache::new()).unwrap();
        // `setup_go_repo` has one function `main` (lines 6-9) with two TODO
        // occurrences inside its body: line 7 (comment) and line 8 (string).
        // The TODO on line 3 is outside any function body; the one on line 12
        // is inside a struct, not a function. So exactly 2 matches.
        assert_eq!(
            result.matches.len(),
            2,
            "golden: exactly 2 TODO matches in function bodies, got {:?}",
            result.matches
        );
        assert_eq!(result.matches[0].line, 7);
        assert_eq!(result.matches[1].line, 8);
    }

    /// Byte-cap test (#52): a file >20MB is skipped pre-read by `ignore`'s
    /// `max_filesize`. The big file contains a unique TODO marker inside a
    /// function body — if it were read, it would produce a match. Asserting
    /// its absence proves the byte-cap is active.
    /// Reverting `max_file_bytes` to `None` REDS this test (the big file is
    /// read and its marker is found).
    #[test]
    fn test_scoped_skips_file_over_20mb() {
        let dir = TempDir::new().unwrap();
        // Normal file with a TODO in a function body.
        fs::write(
            dir.path().join("small.go"),
            "package main\nfunc small() {\n    // TODO_SMALL\n}\n",
        )
        .unwrap();
        // Big file (>20MB) with a unique TODO in a function body, padded with
        // a long block comment to exceed the byte cap.
        let padding = " ".repeat(21 * 1024 * 1024);
        let big_content = format!(
            "package main\nfunc big() {{\n    // TODO_BIG_FILE_MARKER\n}}\n/* {padding} */\n"
        );
        fs::write(dir.path().join("big.go"), big_content).unwrap();

        let input = ScopedSearchInput {
            pattern: "TODO".into(),
            ..go_input(&dir.path().to_string_lossy())
        };
        let result = scoped_search(input, &ScopeCache::new()).unwrap();
        // The small file's TODO must be found.
        assert!(
            result.matches.iter().any(|m| m.text.contains("TODO_SMALL")),
            "small file's TODO must be found"
        );
        // The big file's TODO must NOT be found (byte-cap skipped it).
        assert!(
            !result
                .matches
                .iter()
                .any(|m| m.text.contains("TODO_BIG_FILE_MARKER")),
            "big file (>20MB) must be skipped by the byte cap"
        );
    }

    /// Serde default: an absent `max_files` field deserializes to
    /// `Some(DEFAULT_FILE_COUNT_CAP)` (2000), not `None`.
    #[test]
    fn test_scoped_max_files_default_is_2000() {
        let json = r#"{"root":"/tmp","pattern":"TODO","scope":"function_bodies","language":"go"}"#;
        let input: ScopedSearchInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.max_files, Some(crate::walk::DEFAULT_FILE_COUNT_CAP));
        assert_eq!(input.max_files, Some(2000));
    }

    /// Regression test for issue #48: a same-length in-place edit that lands
    /// within the filesystem's mtime resolution must NOT be served stale.
    ///
    /// The old `(mtime, len)` fingerprint is identical before and after such an
    /// edit, so with TTL=0 (no time-based eviction) the cache would serve the
    /// OLD content permanently. This test writes a file, populates the cache,
    /// then edits the file IN PLACE keeping the exact same byte length and
    /// restoring the original mtime via `FileTimes::set_modified`, then asserts
    /// the second search returns the NEW content (the BBBBBB marker), not the
    /// stale AAAA marker.
    ///
    /// Reverting the content-hash fingerprint (back to mtime+len only) REDS
    /// this test: the second run serves the cached old source and finds AAAA.
    #[test]
    fn test_cache_invalidation_same_length_same_mtime() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("main.go");

        // Original content with marker AAAA.
        let original = "package main\n\nfunc main() {\n    // TODO_AAAAAA\n}\n";
        fs::write(&file_path, original).unwrap();

        // TTL=0 (no time-based eviction) to exercise the worst case: the ONLY
        // staleness guard is the fingerprint itself.
        let cache = ScopeCache::with_capacity_and_ttl(64 * 1024 * 1024, 0);
        let input = go_input(&dir.path().to_string_lossy());

        // Populate the cache.
        let run1 = scoped_search(input.clone(), &cache).unwrap();
        assert!(
            run1.matches.iter().any(|m| m.text.contains("AAAAAA")),
            "first run should find the AAAA marker"
        );

        // Capture the original mtime from the canonical path (the same path
        // the cache key reads metadata from).
        let canonical = std::fs::canonicalize(&file_path).unwrap();
        let original_mtime = std::fs::metadata(&canonical).unwrap().modified().unwrap();

        // Edit in-place: same byte length, different content (AAAA → BBBB).
        let edited = "package main\n\nfunc main() {\n    // TODO_BBBBBB\n}\n";
        assert_eq!(
            original.len(),
            edited.len(),
            "test setup: edited content must have the same byte length"
        );
        fs::write(&canonical, edited).unwrap();

        // Restore the original mtime so the (mtime, len) fingerprint is
        // identical to the pre-edit state.
        let file = std::fs::File::open(&canonical).unwrap();
        let times = std::fs::FileTimes::new().set_modified(original_mtime);
        file.set_times(times).unwrap();

        // Second run: the cache must detect the content change and serve the
        // NEW content, not the stale cached source.
        let run2 = scoped_search(input, &cache).unwrap();
        assert!(
            run2.matches.iter().any(|m| m.text.contains("BBBBBB")),
            "second run must find the NEW BBBBBB marker, not the stale AAAA marker; \
             got {:?}",
            run2.matches
        );
        assert!(
            !run2.matches.iter().any(|m| m.text.contains("AAAAAA")),
            "second run must NOT serve the stale AAAA marker; got {:?}",
            run2.matches
        );
    }
}
