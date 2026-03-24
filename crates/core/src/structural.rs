use std::borrow::Cow;
use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Result};
use ast_grep_core::matcher::{Pattern, PatternBuilder, PatternError};
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::{AstGrep, Language as AstLang};
use ignore::WalkBuilder;
use ox_langs::detect_language;

use crate::grep_filter::build_globset;
use crate::types::{SearchMatch, SearchResponse, StructuralSearchInput};

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
        "java" => Some(LangWrapper::new(
            tree_sitter_java::LANGUAGE.into(),
            EXPANDO,
        )),
        "c" => Some(LangWrapper::new(tree_sitter_c::LANGUAGE.into(), EXPANDO)),
        "cpp" | "c++" | "cxx" => Some(LangWrapper::new(
            tree_sitter_cpp::LANGUAGE.into(),
            EXPANDO,
        )),
        "ruby" | "rb" => Some(LangWrapper::new(
            tree_sitter_ruby::LANGUAGE.into(),
            EXPANDO,
        )),
        "csharp" | "c#" | "cs" => Some(LangWrapper::new(
            tree_sitter_c_sharp::LANGUAGE.into(),
            EXPANDO,
        )),
        "php" => Some(LangWrapper::new(
            tree_sitter_php::LANGUAGE_PHP.into(),
            EXPANDO,
        )),
        "bash" | "sh" => Some(LangWrapper::new(
            tree_sitter_bash::LANGUAGE.into(),
            EXPANDO,
        )),
        "lua" => Some(LangWrapper::new(
            tree_sitter_lua::LANGUAGE.into(),
            EXPANDO,
        )),
        "swift" => Some(LangWrapper::new(
            tree_sitter_swift::LANGUAGE.into(),
            EXPANDO,
        )),
        "kotlin" | "kt" => Some(LangWrapper::new(
            tree_sitter_kotlin_ng::LANGUAGE.into(),
            EXPANDO,
        )),
        "zig" => Some(LangWrapper::new(
            tree_sitter_zig::LANGUAGE.into(),
            EXPANDO,
        )),
        _ => None,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns the file extension mapped to its canonical language name, or None.
/// Used to compare against the requested language (including aliases).
pub(crate) fn file_matches_lang(path: &Path, lang_name: &str) -> bool {
    let detected = path
        .to_str()
        .and_then(detect_language)
        .unwrap_or_default();
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

pub fn structural_search(input: StructuralSearchInput) -> Result<SearchResponse> {
    let start = Instant::now();

    let lang_name = input.language.to_lowercase();
    let wrapper = match lang_wrapper(&lang_name) {
        Some(w) => w,
        None => bail!("unsupported language: {}", input.language),
    };

    // Pre-compile the pattern once; fail fast on invalid patterns.
    let pattern = Pattern::try_new(&input.pattern, wrapper.clone())
        .map_err(|e| anyhow::anyhow!("invalid pattern '{}': {e}", input.pattern))?;

    let include = input.file_glob.as_deref()
        .map(build_globset).transpose()?;
    let exclude = input.exclude_glob.as_deref()
        .map(build_globset).transpose()?;

    let root = Path::new(&input.root);
    let mut all_matches: Vec<SearchMatch> = Vec::new();

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
            all_matches.push(SearchMatch {
                file: rel_path.clone(),
                line,
                text,
                context: vec![],
            });
            if all_matches.len() >= input.max_results {
                break 'files;
            }
        }
    }

    let total_matches = all_matches.len();
    let truncated = total_matches >= input.max_results;

    Ok(SearchResponse {
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
        };
        let result = structural_search(input).unwrap();
        assert!(
            result.matches.len() >= 1,
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
        };
        let result = structural_search(input).unwrap();
        assert_eq!(result.matches.len(), 0);
    }
}
