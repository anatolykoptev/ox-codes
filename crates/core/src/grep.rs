use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};

use crate::grep_filter::{build_globset, lang_extensions};
use crate::types::{ExpandMode, ExpandedMatch, ExpandedSearchResponse, SearchInput, SearchMatch};
use crate::walk::{DEFAULT_MAX_FILE_BYTES, WalkBudget, filtered_walk};

// Test-only thread-local counter: incremented each time a file is read + parsed
// during the expand pass. Tests use this to assert the #46 work-shrink invariant
// (only ≤max_results survivor files are expanded, NOT every matching file).
#[cfg(test)]
thread_local! {
    static EXPAND_FILE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub fn search(input: SearchInput) -> Result<ExpandedSearchResponse> {
    let start = Instant::now();

    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!input.case_sensitive)
        .fixed_strings(!input.is_regex)
        .build(&input.pattern)?;

    let include_globset = input.file_glob.as_deref().map(build_globset).transpose()?;

    let exclude_globset = input
        .exclude_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    // grep's language is Option — an ABSENT language walks all file types
    // (exts: None); a present language restricts to that language's exts.
    let lang_exts: Option<&'static [&'static str]> =
        input.language.as_deref().and_then(lang_extensions);

    let ctx_lines = input.context_lines;
    let root = Path::new(&input.root);

    // Collect (rel_path, matches) per file under the shared filtered walk.
    // The walk is bounded by max_files and the 20MB per-file byte cap.
    let mut per_file: Vec<(String, Vec<SearchMatch>)> = Vec::new();

    let budget = WalkBudget {
        max_files: input.max_files,
        // #52: files >20MB are skipped pre-read by `ignore`'s `max_filesize`.
        max_file_bytes: Some(DEFAULT_MAX_FILE_BYTES),
    };

    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .before_context(ctx_lines)
        .after_context(ctx_lines)
        .build();

    for (path, rel_path, _metadata) in filtered_walk(
        root,
        lang_exts,
        include_globset.as_ref(),
        exclude_globset.as_ref(),
        budget,
    ) {
        let mut sink = CollectSink {
            rel_path: rel_path.clone(),
            matches: Vec::new(),
            context_buf: Vec::new(),
        };

        // Ignore per-file errors (binary files, permission errors, etc.)
        let _ = searcher.search_path(&matcher, &path, &mut sink);

        if !sink.matches.is_empty() {
            per_file.push((rel_path, sink.matches));
        }
    }

    // #46: Compute total_matches as the raw per-file match count BEFORE
    // expansion. This is a plain count — no read/parse needed — and preserves
    // the truncated/count semantics for the common case (max_tokens = None,
    // where no matches are dropped during expansion).
    let total_matches: usize = per_file.iter().map(|(_, m)| m.len()).sum();

    // Rank by match density (stable sort, most matches first).
    per_file.sort_by_key(|a| std::cmp::Reverse(a.1.len()));

    // #46: Select the top `max_results` raw matches in flattened density order
    // and record which (file_idx, match_idx) pairs survive. Only the files
    // containing survivors will be read + parsed in the expand pass — NOT the
    // whole `per_file`. This shrinks the expand work from O(matching-files) to
    // O(≤max_results survivor files).
    let max_results = input.max_results;
    let mut survivors: Vec<(usize, usize)> = Vec::with_capacity(max_results);
    'outer: for (fi, (_, matches)) in per_file.iter().enumerate() {
        for mi in 0..matches.len() {
            survivors.push((fi, mi));
            if survivors.len() >= max_results {
                break 'outer;
            }
        }
    }

    // Group survivors by file so each surviving file is read + parsed once.
    let mut file_to_match_indices: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &(fi, mi) in &survivors {
        file_to_match_indices.entry(fi).or_default().push(mi);
    }

    // Expand only the surviving files. The max_tokens skip stays inside
    // expansion (a survivor dropped by max_tokens is simply absent from the
    // final list — no backfill from lower-ranked files, which would require
    // expanding more files and defeat the #46 work-shrink).
    let do_expand = !matches!(input.expand, ExpandMode::None);
    let mut expanded_map: BTreeMap<(usize, usize), ExpandedMatch> = BTreeMap::new();

    for (fi, match_indices) in &file_to_match_indices {
        let (rel_path, raw_matches) = &per_file[*fi];

        #[cfg(test)]
        EXPAND_FILE_COUNT.with(|c| c.set(c.get() + 1));

        if do_expand {
            let full_path = root.join(rel_path);
            if let Ok(source) = std::fs::read(&full_path)
                && let Some(lang_name) = ox_langs::detect_language(rel_path)
                && let Some(lang_cfg) = ox_langs::get_language(lang_name)
            {
                for &mi in match_indices {
                    let m = &raw_matches[mi];
                    let byte_offset = line_to_byte_offset(&source, m.line);
                    let expanded = crate::expand::find_enclosing_symbol(
                        &source,
                        &lang_cfg.language,
                        byte_offset,
                        &input.expand,
                    );
                    // Skip match if expanded body exceeds max_tokens
                    if let (Some(max_tok), Some(blk)) = (input.max_tokens, &expanded)
                        && blk.body.len() > max_tok
                    {
                        continue;
                    }
                    let expanded = expanded.map(|blk| crate::types::ExpandedBlock {
                        body: crate::expand::wrap_body(
                            blk.body,
                            input.format,
                            input.language.as_deref(),
                        ),
                        ..blk
                    });
                    expanded_map.insert(
                        (*fi, mi),
                        ExpandedMatch {
                            file: m.file.clone(),
                            line: m.line,
                            text: m.text.clone(),
                            context: m.context.clone(),
                            expanded,
                        },
                    );
                }
                continue;
            }
        }

        // No expansion or language not detected — wrap as-is
        for &mi in match_indices {
            let m = &raw_matches[mi];
            expanded_map.insert(
                (*fi, mi),
                ExpandedMatch {
                    file: m.file.clone(),
                    line: m.line,
                    text: m.text.clone(),
                    context: m.context.clone(),
                    expanded: None,
                },
            );
        }
    }

    // Re-flatten survivors in density order, skipping max_tokens-dropped ones.
    let matches: Vec<ExpandedMatch> = survivors
        .into_iter()
        .filter_map(|key| expanded_map.get(&key).cloned())
        .collect();

    let truncated = total_matches > max_results;

    Ok(ExpandedSearchResponse {
        matches,
        total_matches,
        truncated,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return the byte offset of the start of `line_number` (1-based) in `source`.
fn line_to_byte_offset(source: &[u8], line_number: usize) -> usize {
    let mut current_line = 1usize;
    for (i, &byte) in source.iter().enumerate() {
        if current_line == line_number {
            return i;
        }
        if byte == b'\n' {
            current_line += 1;
        }
    }
    0
}

// ── Sink implementation ───────────────────────────────────────────────────────

struct CollectSink {
    rel_path: String,
    matches: Vec<SearchMatch>,
    context_buf: Vec<String>,
}

impl Sink for CollectSink {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let text = String::from_utf8_lossy(mat.bytes()).trim_end().to_string();
        let line = mat.line_number().unwrap_or(0) as usize;
        let context = std::mem::take(&mut self.context_buf);
        self.matches.push(SearchMatch {
            file: self.rel_path.clone(),
            line,
            text,
            context,
        });
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        let text = String::from_utf8_lossy(ctx.bytes()).trim_end().to_string();
        self.context_buf.push(text);
        Ok(true)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExpandMode;
    use std::fs;
    use tempfile::TempDir;

    fn make_input(root: &str, pattern: &str) -> SearchInput {
        SearchInput {
            root: root.to_string(),
            pattern: pattern.to_string(),
            is_regex: false,
            file_glob: None,
            exclude_glob: None,
            context_lines: 0,
            max_results: 50,
            case_sensitive: true,
            language: None,
            expand: ExpandMode::default(),
            max_tokens: None,
            format: crate::types::Format::Plain,
            max_files: Some(crate::walk::DEFAULT_FILE_COUNT_CAP),
        }
    }

    fn write_file(dir: &TempDir, name: &str, content: &str) {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn test_literal_search() {
        let dir = TempDir::new().unwrap();
        write_file(
            &dir,
            "main.go",
            "func HandleRequest(w http.ResponseWriter) {}",
        );
        let resp = search(make_input(dir.path().to_str().unwrap(), "HandleRequest")).unwrap();
        assert_eq!(resp.matches.len(), 1);
        assert!(resp.matches[0].text.contains("HandleRequest"));
    }

    #[test]
    fn test_regex_search() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.go", "func Foo() {}\nfunc Bar() {}");
        let mut inp = make_input(dir.path().to_str().unwrap(), r"func \w+");
        inp.is_regex = true;
        let resp = search(inp).unwrap();
        assert_eq!(resp.matches.len(), 2);
    }

    #[test]
    fn test_case_insensitive() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.go", "func handleRequest() {}");
        let mut inp = make_input(dir.path().to_str().unwrap(), "HandleRequest");
        inp.case_sensitive = false;
        let resp = search(inp).unwrap();
        assert_eq!(resp.matches.len(), 1);
    }

    #[test]
    fn test_file_glob() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.go", "target here");
        write_file(&dir, "readme.txt", "target here");
        let mut inp = make_input(dir.path().to_str().unwrap(), "target");
        inp.file_glob = Some("*.go".to_string());
        let resp = search(inp).unwrap();
        assert_eq!(resp.matches.len(), 1);
        assert!(resp.matches[0].file.ends_with(".go"));
    }

    #[test]
    fn test_exclude_glob() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.go", "target here");
        write_file(&dir, "vendor/dep.go", "target here");
        let mut inp = make_input(dir.path().to_str().unwrap(), "target");
        inp.exclude_glob = Some("vendor/*".to_string());
        let resp = search(inp).unwrap();
        assert_eq!(resp.matches.len(), 1);
        assert!(!resp.matches[0].file.starts_with("vendor"));
    }

    #[test]
    fn test_context_lines() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "a.go", "line1\nTARGET\nline3");
        let mut inp = make_input(dir.path().to_str().unwrap(), "TARGET");
        inp.context_lines = 1;
        let resp = search(inp).unwrap();
        assert_eq!(resp.matches.len(), 1);
        assert!(!resp.matches[0].context.is_empty());
    }

    #[test]
    fn test_max_results() {
        let dir = TempDir::new().unwrap();
        let content: String = (0..20).map(|i| format!("match line {i}\n")).collect();
        write_file(&dir, "a.go", &content);
        let mut inp = make_input(dir.path().to_str().unwrap(), "match line");
        inp.max_results = 5;
        let resp = search(inp).unwrap();
        assert_eq!(resp.matches.len(), 5);
        assert!(resp.truncated);
        assert_eq!(resp.total_matches, 20);
    }

    #[test]
    fn test_empty_results() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "a.go", "nothing here");
        let resp = search(make_input(dir.path().to_str().unwrap(), "ZZZNOMATCH")).unwrap();
        assert!(resp.matches.is_empty());
        assert!(!resp.truncated);
    }

    #[test]
    fn test_density_ranking() {
        let dir = TempDir::new().unwrap();
        // b.go has 3 matches, a.go has 1 — b.go should appear first
        write_file(&dir, "a.go", "hit here");
        write_file(&dir, "b.go", "hit one\nhit two\nhit three");
        let resp = search(make_input(dir.path().to_str().unwrap(), "hit")).unwrap();
        assert!(resp.matches.len() >= 4);
        assert_eq!(resp.matches[0].file, "b.go");
    }

    #[test]
    fn test_grep_with_expand_function() {
        let dir = TempDir::new().unwrap();
        write_file(
            &dir,
            "main.go",
            "package main\n\nfunc handler() {\n    doQuery()\n}\n",
        );
        let mut inp = make_input(dir.path().to_str().unwrap(), "doQuery");
        inp.expand = ExpandMode::Function;
        let resp = search(inp).unwrap();
        assert_eq!(resp.matches.len(), 1);
        let block = resp.matches[0].expanded.as_ref().unwrap();
        assert_eq!(block.symbol_name, "handler");
        assert!(block.body.contains("doQuery()"));
    }

    #[test]
    fn test_expand_markdown_wraps_body() {
        let dir = TempDir::new().unwrap();
        write_file(
            &dir,
            "main.go",
            "package main\nfunc Foo() { x := 1\n_ = x\n}\n",
        );
        let inp = SearchInput {
            root: dir.path().to_str().unwrap().into(),
            pattern: "Foo".into(),
            is_regex: false,
            file_glob: None,
            exclude_glob: None,
            context_lines: 0,
            max_results: 50,
            case_sensitive: true,
            language: Some("go".into()),
            expand: ExpandMode::Function,
            max_tokens: None,
            format: crate::types::Format::Markdown,
            max_files: Some(crate::walk::DEFAULT_FILE_COUNT_CAP),
        };
        let resp = search(inp).unwrap();
        assert!(!resp.matches.is_empty(), "expected match");
        let body = &resp.matches[0].expanded.as_ref().unwrap().body;
        assert!(
            body.starts_with("```go"),
            "expected markdown fence, got: {}",
            &body[..body.len().min(50)]
        );
        assert!(body.ends_with("```"), "expected closing fence");
    }

    // ── PR2: filtered_walk adoption + #46 reorder tests ────────────────

    /// Golden / result-identity test: adopting `filtered_walk` and `WalkBudget`
    /// and the #46 reorder (expand only survivors) did NOT change what
    /// `/search` returns for a normal request with expansion. The match
    /// count, file order (density-ranked), line numbers, and expanded symbol
    /// names must be exactly what the pre-reorder code produced.
    ///
    /// Fixture: b.go has 2 TARGET matches (density=2), a.go has 1 (density=1).
    /// Density sort → b.go first. With max_results=50 all 3 survive. Expand
    /// pass reads + parses both survivor files and expands to enclosing
    /// functions (beta, alpha).
    #[test]
    fn test_search_golden_result_identity() {
        let dir = TempDir::new().unwrap();
        write_file(
            &dir,
            "a.go",
            "package main\nfunc alpha() {\n    TARGET\n}\n",
        );
        write_file(
            &dir,
            "b.go",
            "package main\nfunc beta() {\n    TARGET one\n    TARGET two\n}\n",
        );

        let mut inp = make_input(dir.path().to_str().unwrap(), "TARGET");
        inp.language = Some("go".into());
        inp.expand = ExpandMode::Function;
        let resp = search(inp).unwrap();

        // total_matches = 3 (raw count before expansion), not truncated.
        assert_eq!(resp.total_matches, 3);
        assert!(!resp.truncated);
        assert_eq!(resp.matches.len(), 3);

        // Density order: b.go (2 matches) before a.go (1 match).
        assert_eq!(resp.matches[0].file, "b.go");
        assert_eq!(resp.matches[0].line, 3);
        assert_eq!(
            resp.matches[0].expanded.as_ref().unwrap().symbol_name,
            "beta"
        );
        assert_eq!(resp.matches[1].file, "b.go");
        assert_eq!(resp.matches[1].line, 4);
        assert_eq!(
            resp.matches[1].expanded.as_ref().unwrap().symbol_name,
            "beta"
        );
        assert_eq!(resp.matches[2].file, "a.go");
        assert_eq!(resp.matches[2].line, 3);
        assert_eq!(
            resp.matches[2].expanded.as_ref().unwrap().symbol_name,
            "alpha"
        );
    }

    /// #46 work-shrink test: with matches in ≫max_results files and a small
    /// `max_results`, the expand pass must read + parse ONLY the ≤max_results
    /// survivor files — NOT every matching file. The thread-local
    /// `EXPAND_FILE_COUNT` counter is incremented once per file expanded.
    ///
    /// Reverting the #46 reorder (expanding the whole `per_file`) REDS this
    /// test: EXPAND_FILE_COUNT jumps from 3 to 20 (all matching files).
    #[test]
    fn test_search_expands_only_top_results() {
        let dir = TempDir::new().unwrap();
        // 20 files, each with exactly 1 match — all density-equal, so the
        // stable sort preserves path order (f00..f19).
        for i in 0..20 {
            let name = format!("f{i:02}.go");
            write_file(&dir, &name, "package main\nfunc f() {\n    TARGET\n}\n");
        }

        let mut inp = make_input(dir.path().to_str().unwrap(), "TARGET");
        inp.language = Some("go".into());
        inp.expand = ExpandMode::Function;
        inp.max_results = 3;

        EXPAND_FILE_COUNT.with(|c| c.set(0));
        let resp = search(inp).unwrap();

        let expanded_files = EXPAND_FILE_COUNT.with(|c| c.get());

        // All 20 files matched → total_matches = 20, truncated.
        assert_eq!(resp.total_matches, 20);
        assert!(resp.truncated);
        // Only 3 matches returned (max_results).
        assert_eq!(resp.matches.len(), 3);

        // #46 invariant: at most max_results files were expanded.
        assert!(
            expanded_files <= 3,
            "expand pass should read+parse ≤max_results=3 survivor files, \
             got {expanded_files} (pre-#46 would expand all 20)"
        );
        // And at least 1 (the survivors must be expanded to produce results).
        assert!(
            expanded_files >= 1,
            "at least one survivor file must be expanded"
        );
    }

    /// Byte-cap test (#52): a file >20MB is skipped pre-read by `ignore`'s
    /// `max_filesize`. The big file contains a unique marker — if it were
    /// read, it would produce a match. Asserting its absence proves the
    /// byte-cap is active.
    /// Reverting `max_file_bytes` to `None` REDS this test (the big file is
    /// read and its marker is found).
    #[test]
    fn test_search_skips_file_over_20mb() {
        let dir = TempDir::new().unwrap();
        // Normal file with a TARGET marker.
        write_file(
            &dir,
            "small.go",
            "package main\nfunc small() {\n    TARGET_SMALL\n}\n",
        );
        // Big file (>20MB) with a unique TARGET marker, padded to exceed the
        // byte cap.
        let padding = " ".repeat(21 * 1024 * 1024);
        let big_content = format!(
            "package main\nfunc big() {{\n    TARGET_BIG_FILE_MARKER\n}}\n/* {padding} */\n"
        );
        write_file(&dir, "big.go", &big_content);

        let mut inp = make_input(dir.path().to_str().unwrap(), "TARGET");
        inp.language = Some("go".into());
        let resp = search(inp).unwrap();

        // The small file's TARGET must be found.
        assert!(
            resp.matches.iter().any(|m| m.text.contains("TARGET_SMALL")),
            "small file's TARGET must be found"
        );
        // The big file's TARGET must NOT be found (byte-cap skipped it).
        assert!(
            !resp
                .matches
                .iter()
                .any(|m| m.text.contains("TARGET_BIG_FILE_MARKER")),
            "big file (>20MB) must be skipped by the byte cap"
        );
    }

    /// Serde default: an absent `max_files` field deserializes to
    /// `Some(DEFAULT_FILE_COUNT_CAP)` (2000), not `None`.
    #[test]
    fn test_search_max_files_default_is_2000() {
        let json = r#"{"root":"/tmp","pattern":"TODO"}"#;
        let input: SearchInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.max_files, Some(crate::walk::DEFAULT_FILE_COUNT_CAP));
        assert_eq!(input.max_files, Some(2000));
    }
}
