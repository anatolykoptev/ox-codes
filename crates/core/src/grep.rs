use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::WalkBuilder;

use crate::types::{SearchInput, SearchMatch, SearchResponse};
use crate::grep_filter::{build_globset, lang_extensions, matches_language};

pub fn search(input: SearchInput) -> Result<SearchResponse> {
    let start = Instant::now();

    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!input.case_sensitive)
        .fixed_strings(!input.is_regex)
        .build(&input.pattern)?;

    let include_globset = input
        .file_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    let exclude_globset = input
        .exclude_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    let lang_exts: Option<&[&str]> = input
        .language
        .as_deref()
        .and_then(lang_extensions);

    let ctx_lines = input.context_lines;
    let root = Path::new(&input.root);

    // Collect (file_path, matches) per file
    let mut per_file: Vec<(String, Vec<SearchMatch>)> = Vec::new();

    let walker = WalkBuilder::new(root)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()));

    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .before_context(ctx_lines)
        .after_context(ctx_lines)
        .build();

    for entry in walker {
        let path = entry.path();
        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();

        // Language filter
        if let Some(exts) = lang_exts
            && !matches_language(&rel_path, exts)
        {
            continue;
        }

        // Include glob filter
        if let Some(ref gs) = include_globset
            && !gs.is_match(&rel_path)
        {
            continue;
        }

        // Exclude glob filter
        if let Some(ref gs) = exclude_globset
            && gs.is_match(&rel_path)
        {
            continue;
        }

        let mut sink = CollectSink {
            rel_path: rel_path.clone(),
            matches: Vec::new(),
            context_buf: Vec::new(),
        };

        // Ignore per-file errors (binary files, permission errors, etc.)
        let _ = searcher.search_path(&matcher, path, &mut sink);

        if !sink.matches.is_empty() {
            per_file.push((rel_path, sink.matches));
        }
    }

    // Rank by match density (stable sort, most matches first)
    per_file.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    // Flatten and truncate
    let all_matches: Vec<SearchMatch> = per_file
        .into_iter()
        .flat_map(|(_, ms)| ms)
        .collect();

    let total_matches = all_matches.len();
    let truncated = total_matches > input.max_results;
    let matches = all_matches.into_iter().take(input.max_results).collect();

    Ok(SearchResponse {
        matches,
        total_matches,
        truncated,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

// ── Sink implementation ───────────────────────────────────────────────────────

struct CollectSink {
    rel_path: String,
    matches: Vec<SearchMatch>,
    context_buf: Vec<String>,
}

impl Sink for CollectSink {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
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
        write_file(&dir, "main.go", "func HandleRequest(w http.ResponseWriter) {}");
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
}
