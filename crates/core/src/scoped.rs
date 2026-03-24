use std::path::Path;
use std::time::Instant;
use anyhow::Result;
use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use ignore::WalkBuilder;
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::grep_filter::build_globset;
use crate::types::{ScopedSearchInput, SearchMatch, SearchResponse};
use ox_langs::{get_language, get_scope_query, ScopeKind};

pub fn scoped_search(input: ScopedSearchInput) -> Result<SearchResponse> {
    let start = Instant::now();

    let scope = parse_scope_kind(&input.scope)?;

    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!input.case_sensitive)
        .build(&input.pattern)?;

    let lang_cfg = get_language(&input.language)
        .ok_or_else(|| anyhow::anyhow!("unsupported language: {}", input.language))?;
    let query_str = get_scope_query(&input.language, scope)
        .ok_or_else(|| anyhow::anyhow!("no scope query for {}/{:?}", input.language, scope))?;

    let query = Query::new(&lang_cfg.language, query_str)?;

    let include = input.file_glob.as_deref()
        .map(build_globset).transpose()?;
    let exclude = input.exclude_glob.as_deref()
        .map(build_globset).transpose()?;

    let mut all_matches = Vec::new();
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
        let source = match std::fs::read(path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let file_matches = search_in_scopes(
            &source,
            &lang_cfg.language,
            &query,
            &matcher,
            &rel_path.to_string_lossy(),
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

    Ok(SearchResponse {
        matches: all_matches,
        total_matches: total,
        truncated,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

fn search_in_scopes(
    source: &[u8],
    language: &tree_sitter::Language,
    query: &Query,
    matcher: &grep_regex::RegexMatcher,
    rel_path: &str,
) -> Vec<SearchMatch> {
    let mut parser = Parser::new();
    if parser.set_language(language).is_err() {
        return vec![];
    }
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return vec![],
    };

    let mut cursor = QueryCursor::new();
    let mut matches = Vec::new();

    let mut query_matches = cursor.matches(query, tree.root_node(), source);
    while let Some(qmatch) = query_matches.next() {
        for capture in qmatch.captures {
            let node = capture.node;
            let scope_text = &source[node.byte_range()];
            let scope_start_line = node.start_position().row + 1; // 1-indexed

            let scope_str = String::from_utf8_lossy(scope_text);
            for (line_offset, line) in scope_str.lines().enumerate() {
                if matcher.is_match(line.as_bytes()).unwrap_or(false) {
                    matches.push(SearchMatch {
                        file: rel_path.to_string(),
                        line: scope_start_line + line_offset,
                        text: line.trim_end().to_string(),
                        context: vec![],
                    });
                }
            }
        }
    }

    matches
}

fn parse_scope_kind(s: &str) -> Result<ScopeKind> {
    match s {
        "function_bodies" | "functions" => Ok(ScopeKind::FunctionBodies),
        "comments" => Ok(ScopeKind::Comments),
        "strings" => Ok(ScopeKind::Strings),
        "type_definitions" | "types" => Ok(ScopeKind::TypeDefinitions),
        "imports" => Ok(ScopeKind::Imports),
        _ => anyhow::bail!(
            "unknown scope kind: {s}. Valid: function_bodies, comments, strings, type_definitions, imports"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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

    #[test]
    fn test_scoped_function_bodies() {
        let dir = setup_go_repo();
        let input = ScopedSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "TODO".into(),
            scope: "function_bodies".into(),
            language: "go".into(),
            is_regex: false,
            max_results: 50,
            case_sensitive: true,
            file_glob: None,
            exclude_glob: None,
        };
        let result = scoped_search(input).unwrap();
        // Only TODOs inside function bodies (line 7 comment, line 8 string)
        assert!(result.matches.len() >= 1);
        assert!(result.matches.iter().all(|m| m.line >= 6)); // inside main()
    }

    #[test]
    fn test_scoped_comments() {
        let dir = setup_go_repo();
        let input = ScopedSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "TODO".into(),
            scope: "comments".into(),
            language: "go".into(),
            is_regex: false,
            max_results: 50,
            case_sensitive: true,
            file_glob: None,
            exclude_glob: None,
        };
        let result = scoped_search(input).unwrap();
        // Comments with TODO: line 3, line 7, line 12
        assert!(result.matches.len() >= 2);
    }

    #[test]
    fn test_scoped_strings() {
        let dir = setup_go_repo();
        let input = ScopedSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "TODO".into(),
            scope: "strings".into(),
            language: "go".into(),
            is_regex: false,
            max_results: 50,
            case_sensitive: true,
            file_glob: None,
            exclude_glob: None,
        };
        let result = scoped_search(input).unwrap();
        // Only "hello TODO" string
        assert_eq!(result.matches.len(), 1);
    }

    #[test]
    fn test_invalid_scope() {
        let dir = setup_go_repo();
        let input = ScopedSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "TODO".into(),
            scope: "invalid_scope".into(),
            language: "go".into(),
            is_regex: false,
            max_results: 50,
            case_sensitive: true,
            file_glob: None,
            exclude_glob: None,
        };
        assert!(scoped_search(input).is_err());
    }
}
