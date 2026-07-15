use std::borrow::Cow;
use std::path::Path;
use std::time::Instant;

use anyhow::{Result, bail};
use ast_grep_core::matcher::{Pattern, PatternBuilder, PatternError};
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::{AstGrep, Language as AstLang, Matcher};
use ignore::WalkBuilder;
use ox_langs::detect_language;

use crate::grep_filter::build_globset;
use crate::types::{ExpandMode, ExpandedMatch, ExpandedSearchResponse, StructuralSearchInput};

// ── Language wrapper ──────────────────────────────────────────────────────────

/// Wraps a tree-sitter language for use with ast-grep-core.
#[derive(Clone)]
pub(crate) struct LangWrapper {
    ts_lang: TSLanguage,
    /// Languages that don't accept `$` as identifier start need a different
    /// expando char so that `$VAR` patterns parse correctly.
    expando: char,
}

impl LangWrapper {
    fn new(ts_lang: TSLanguage, expando: char) -> Self {
        Self { ts_lang, expando }
    }
}

impl AstLang for LangWrapper {
    fn meta_var_char(&self) -> char {
        '$'
    }

    fn expando_char(&self) -> char {
        self.expando
    }

    /// When expando != '$', replace `$VAR` with `<expando>VAR` so that the
    /// language parser can treat it as a valid identifier.
    fn pre_process_pattern<'q>(&self, query: &'q str) -> Cow<'q, str> {
        if self.expando == '$' {
            return Cow::Borrowed(query);
        }
        let expando = self.expando;
        let mut out = Vec::with_capacity(query.len());
        let mut dollar_count = 0usize;
        for c in query.chars() {
            if c == '$' {
                dollar_count += 1;
                continue;
            }
            // `$VAR`, `$$VAR`, `$$$VAR` (ellipsis), `$$$` (anonymous multi)
            let needs_replace = matches!(c, 'A'..='Z' | '_') || dollar_count == 3;
            let sigil = if needs_replace { expando } else { '$' };
            for _ in 0..dollar_count {
                out.push(sigil);
            }
            dollar_count = 0;
            out.push(c);
        }
        // trailing `$$$`
        let sigil = if dollar_count == 3 { expando } else { '$' };
        for _ in 0..dollar_count {
            out.push(sigil);
        }
        Cow::Owned(out.into_iter().collect())
    }

    fn kind_to_id(&self, kind: &str) -> u16 {
        self.ts_lang.id_for_node_kind(kind, true)
    }

    fn field_to_id(&self, field: &str) -> Option<u16> {
        self.ts_lang.field_id_for_name(field).map(|f| f.get())
    }

    fn build_pattern(&self, builder: &PatternBuilder) -> Result<Pattern, PatternError> {
        builder.build(|src| StrDoc::try_new(src, self.clone()))
    }
}

impl LanguageExt for LangWrapper {
    fn get_ts_language(&self) -> TSLanguage {
        self.ts_lang.clone()
    }
}

// ── Language selection ────────────────────────────────────────────────────────

/// Returns a LangWrapper for the given (already lower-cased) language name.
/// `µ` is used as the expando char for languages that don't accept `$` in
/// identifiers; TypeScript/JS use `$` natively.
pub(crate) fn lang_wrapper(name: &str) -> Option<LangWrapper> {
    const EXPANDO: char = 'µ';
    match name {
        "go" | "golang" => Some(LangWrapper::new(tree_sitter_go::LANGUAGE.into(), EXPANDO)),
        "rust" | "rs" => Some(LangWrapper::new(tree_sitter_rust::LANGUAGE.into(), EXPANDO)),
        "python" | "py" => Some(LangWrapper::new(
            tree_sitter_python::LANGUAGE.into(),
            EXPANDO,
        )),
        "typescript" | "ts" | "javascript" | "js" => Some(LangWrapper::new(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            '$',
        )),
        "tsx" => Some(LangWrapper::new(
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            '$',
        )),
        "java" => Some(LangWrapper::new(tree_sitter_java::LANGUAGE.into(), EXPANDO)),
        "c" => Some(LangWrapper::new(tree_sitter_c::LANGUAGE.into(), EXPANDO)),
        "cpp" | "c++" | "cxx" => Some(LangWrapper::new(tree_sitter_cpp::LANGUAGE.into(), EXPANDO)),
        "ruby" | "rb" => Some(LangWrapper::new(tree_sitter_ruby::LANGUAGE.into(), EXPANDO)),
        "csharp" | "c#" | "cs" => Some(LangWrapper::new(
            tree_sitter_c_sharp::LANGUAGE.into(),
            EXPANDO,
        )),
        "php" => Some(LangWrapper::new(
            tree_sitter_php::LANGUAGE_PHP.into(),
            EXPANDO,
        )),
        "bash" | "sh" => Some(LangWrapper::new(tree_sitter_bash::LANGUAGE.into(), EXPANDO)),
        "lua" => Some(LangWrapper::new(tree_sitter_lua::LANGUAGE.into(), EXPANDO)),
        "swift" => Some(LangWrapper::new(
            tree_sitter_swift::LANGUAGE.into(),
            EXPANDO,
        )),
        "kotlin" | "kt" => Some(LangWrapper::new(
            tree_sitter_kotlin_ng::LANGUAGE.into(),
            EXPANDO,
        )),
        "zig" => Some(LangWrapper::new(tree_sitter_zig::LANGUAGE.into(), EXPANDO)),
        _ => None,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns the file extension mapped to its canonical language name, or None.
/// Used to compare against the requested language (including aliases).
pub(crate) fn file_matches_lang(path: &Path, lang_name: &str) -> bool {
    let detected = path.to_str().and_then(detect_language).unwrap_or_default();
    if detected == lang_name {
        return true;
    }
    // Handle aliases
    matches!(
        (lang_name, detected),
        ("golang", "go")
            | ("rs", "rust")
            | ("py", "python")
            | ("ts", "typescript")
            | ("js", "typescript")
            | ("javascript", "typescript")
            | ("c++", "cpp")
            | ("cxx", "cpp")
            | ("rb", "ruby")
            | ("c#", "csharp")
            | ("cs", "csharp")
            | ("sh", "bash")
            | ("kt", "kotlin")
    )
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Build an ast-grep `Pattern` for a Go query, handling the Go-specific
/// ambiguity where `X.Y(args)` at file scope is parsed as
/// `type_conversion_expression` (package.Type(value)) instead of a
/// `call_expression`.  We detect this by checking `potential_kinds()` after a
/// first parse attempt, and if the result is a type-conversion we re-parse
/// using `Pattern::contextual` with a function-body wrapper so that the
/// fragment is unambiguously an expression statement.
///
/// Patterns that do NOT mis-parse (ERROR nodes, plain function calls, etc.)
/// are returned as-is from the first attempt.
fn build_pattern_for_go(query: &str, wrapper: &LangWrapper) -> Result<Pattern> {
    let pattern = Pattern::try_new(query, wrapper.clone())
        .map_err(|e| anyhow::anyhow!("invalid pattern '{query}': {e}"))?;

    // `type_conversion_expression` is the tell: Go misparses `X.Y(args)` as
    // `package.Type(value)`.  If the pattern anchors on that kind, it will
    // never match call_expression nodes in real code.
    let tc_kind = wrapper.kind_to_id("type_conversion_expression") as usize;
    let is_misparse = pattern
        .potential_kinds()
        .map(|k| k.contains(tc_kind))
        .unwrap_or(false);

    if !is_misparse {
        return Ok(pattern);
    }

    // Re-parse inside a function body so the fragment is an expression
    // statement.  Pattern::contextual handles pre_process_pattern internally.
    let wrapped = format!("func _() {{ {query} }}");
    Pattern::contextual(&wrapped, "call_expression", wrapper.clone())
        .map_err(|e| anyhow::anyhow!("invalid pattern '{query}': {e}"))
}

pub fn structural_search(input: StructuralSearchInput) -> Result<ExpandedSearchResponse> {
    let start = Instant::now();

    let lang_name = input.language.to_lowercase();
    let wrapper = match lang_wrapper(&lang_name) {
        Some(w) => w,
        None => bail!("unsupported language: {}", input.language),
    };

    // Pre-compile the pattern once; fail fast on invalid patterns.
    // For Go, use a specialised builder that handles selector-expression
    // ambiguity (X.Y(args) misparses as type_conversion_expression at file scope).
    let pattern = if matches!(lang_name.as_str(), "go" | "golang") {
        build_pattern_for_go(&input.pattern, &wrapper)
    } else {
        Pattern::try_new(&input.pattern, wrapper.clone())
            .map_err(|e| anyhow::anyhow!("invalid pattern '{}': {e}", input.pattern))
    }?;

    let include = input.file_glob.as_deref().map(build_globset).transpose()?;
    let exclude = input
        .exclude_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    let root = Path::new(&input.root);
    let mut all_matches: Vec<ExpandedMatch> = Vec::new();

    // Get tree-sitter language for expand (if needed).
    let ts_lang = if !matches!(input.expand, ExpandMode::None) {
        ox_langs::get_language(&lang_name).map(|cfg| cfg.language)
    } else {
        None
    };

    let walker = WalkBuilder::new(root)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()));

    'files: for entry in walker {
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
            Err(_) => continue, // skip binary / unreadable files
        };

        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();

        let ast: AstGrep<StrDoc<LangWrapper>> = AstGrep::new(&src, wrapper.clone());

        for node_match in ast.root().find_all(pattern.clone()) {
            // start_pos().line() is 0-based; convert to 1-based for output.
            let line = node_match.start_pos().line() + 1;
            // Only include the first line of multi-line matches for readability.
            let text = match node_match.text() {
                Cow::Borrowed(s) => first_line(s),
                Cow::Owned(ref s) => first_line(s),
            };

            // Compute byte offset for expand.
            let byte_pos = node_match.range().start;

            // Optionally expand to enclosing symbol.
            let expanded = if let Some(ref lang) = ts_lang {
                let block = crate::expand::find_enclosing_symbol(
                    src.as_bytes(),
                    lang,
                    byte_pos,
                    &input.expand,
                );
                // If max_tokens is set, skip matches whose expanded body is too large.
                if let (Some(b), Some(max_tok)) = (&block, input.max_tokens) {
                    let estimated_tokens = b.body.len() / 4;
                    if estimated_tokens > max_tok {
                        continue;
                    }
                }
                block.map(|blk| crate::types::ExpandedBlock {
                    body: crate::expand::wrap_body(blk.body, input.format, Some(&input.language)),
                    ..blk
                })
            } else {
                None
            };

            all_matches.push(ExpandedMatch {
                file: rel_path.clone(),
                line,
                text,
                context: vec![],
                expanded,
            });
            if all_matches.len() >= input.max_results {
                break 'files;
            }
        }
    }

    let total_matches = all_matches.len();
    let truncated = total_matches >= input.max_results;

    Ok(ExpandedSearchResponse {
        matches: all_matches,
        total_matches,
        truncated,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExpandMode;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_structural_go_error_pattern() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            r#"package main

func foo() error {
    val, err := doSomething()
    if err != nil {
        return err
    }
    if val == nil {
        return nil
    }
    return nil
}
"#,
        )
        .unwrap();
        let input = StructuralSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "if $ERR != nil { return $ERR }".into(),
            language: "go".into(),
            max_results: 50,
            file_glob: None,
            exclude_glob: None,
            expand: ExpandMode::default(),
            max_tokens: None,
            format: crate::types::Format::Plain,
        };
        let result = structural_search(input).unwrap();
        assert!(
            !result.matches.is_empty(),
            "should find error check pattern, got: {:?}",
            result.matches
        );
    }

    #[test]
    fn test_structural_no_match() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            "package main\n\nfunc main() {}\n",
        )
        .unwrap();
        let input = StructuralSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "if $X != nil { return $X }".into(),
            language: "go".into(),
            max_results: 50,
            file_glob: None,
            exclude_glob: None,
            expand: ExpandMode::default(),
            max_tokens: None,
            format: crate::types::Format::Plain,
        };
        let result = structural_search(input).unwrap();
        assert_eq!(result.matches.len(), 0);
    }

    #[test]
    fn test_structural_with_expand_function() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            r#"package main

func handler() error {
    val, err := db.Query()
    if err != nil {
        return err
    }
    return nil
}

func other() {
    fmt.Println("hello")
}
"#,
        )
        .unwrap();
        let input = StructuralSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "if $ERR != nil { return $ERR }".into(),
            language: "go".into(),
            max_results: 50,
            file_glob: None,
            exclude_glob: None,
            expand: ExpandMode::Function,
            max_tokens: None,
            format: crate::types::Format::Plain,
        };
        let result = structural_search(input).unwrap();
        assert_eq!(result.matches.len(), 1);
        let m = &result.matches[0];
        assert!(m.expanded.is_some());
        let block = m.expanded.as_ref().unwrap();
        assert_eq!(block.symbol_name, "handler");
        assert!(block.body.contains("db.Query()"));
        assert!(block.body.contains("return err"));
    }

    /// Regression test: `$RECV.Method($$$)` matches `s.BulkCopyInsert(ctx, a, b)`.
    ///
    /// Previously this returned 0 matches because the Go grammar mis-parsed
    /// `X.Y(args)` at file scope as `type_conversion_expression` (package.Type(value))
    /// instead of `call_expression`.  `build_pattern_for_go` detects this and
    /// re-parses via `Pattern::contextual` inside a function-body wrapper.
    #[test]
    fn test_structural_go_method_call_limitation() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            "package main\nfunc f(s *Store) { s.BulkCopyInsert(ctx, a, b) }\n",
        )
        .unwrap();
        let input = StructuralSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "$RECV.BulkCopyInsert($$$)".into(),
            language: "go".into(),
            max_results: 5,
            file_glob: None,
            exclude_glob: None,
            expand: ExpandMode::default(),
            max_tokens: None,
            format: crate::types::Format::Plain,
        };
        let result = structural_search(input).unwrap();
        assert!(
            !result.matches.is_empty(),
            "$RECV.Method($$$) should match multi-arg method call"
        );
    }

    /// Additional regression: `$RECV.BulkInsert($$$)` matches `s.BulkInsert(ctx, a, b)`.
    #[test]
    fn test_structural_go_method_call_with_ellipsis() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            "package main\nfunc f(s *Store) { s.BulkInsert(ctx, a, b) }\n",
        )
        .unwrap();
        let input = StructuralSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "$RECV.BulkInsert($$$)".into(),
            language: "go".into(),
            max_results: 5,
            file_glob: None,
            exclude_glob: None,
            expand: ExpandMode::default(),
            max_tokens: None,
            format: crate::types::Format::Plain,
        };
        let result = structural_search(input).unwrap();
        assert!(
            !result.matches.is_empty(),
            "method call with ellipsis should match"
        );
    }

    /// Verify that plain function calls with ellipsis work correctly.
    /// `foo($$$)` matches `foo(ctx, a, b)` — ellipsis in plain (non-method) calls is fine.
    #[test]
    fn test_structural_go_plain_func_ellipsis() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            "package main\nfunc f() { foo(ctx, a, b) }\n",
        )
        .unwrap();
        let input = StructuralSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "foo($$$)".into(),
            language: "go".into(),
            max_results: 5,
            file_glob: None,
            exclude_glob: None,
            expand: ExpandMode::default(),
            max_tokens: None,
            format: crate::types::Format::Plain,
        };
        let result = structural_search(input).unwrap();
        assert!(
            !result.matches.is_empty(),
            "foo($$$) should match foo(ctx, a, b)"
        );
    }

    /// Verify that `$RECV.Method()` (no-arg call) works despite the selector_expression issue.
    #[test]
    fn test_structural_go_method_no_args() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            "package main\nfunc f(s *Store) { s.Close() }\n",
        )
        .unwrap();
        let input = StructuralSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "$RECV.Close()".into(),
            language: "go".into(),
            max_results: 5,
            file_glob: None,
            exclude_glob: None,
            expand: ExpandMode::default(),
            max_tokens: None,
            format: crate::types::Format::Plain,
        };
        let result = structural_search(input).unwrap();
        assert!(
            !result.matches.is_empty(),
            "$RECV.Method() should match no-arg method call"
        );
    }

    /// Workaround test: `$RECV.Method($X, $$$)` matches method calls with 1+ arguments.
    /// This is the recommended pattern when you need to match multi-arg method calls.
    #[test]
    fn test_structural_go_method_call_one_plus_args() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.go"),
            "package main\nfunc f(s *Store) { s.Insert(ctx, a, b) }\n",
        )
        .unwrap();
        // Method + at least 1 arg pattern: $RECV.Method($X, $$$) matches multi-arg calls
        let input = StructuralSearchInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "$RECV.Insert($X, $$$)".into(),
            language: "go".into(),
            max_results: 5,
            file_glob: None,
            exclude_glob: None,
            expand: ExpandMode::default(),
            max_tokens: None,
            format: crate::types::Format::Plain,
        };
        let result = structural_search(input).unwrap();
        assert!(
            !result.matches.is_empty(),
            "$RECV.Method($X, $$$) should match multi-arg method call"
        );
    }
}
