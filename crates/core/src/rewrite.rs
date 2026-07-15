use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use anyhow::{Result, bail};
use ast_grep_core::AstGrep;
use ast_grep_core::matcher::Pattern;
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc};
use ignore::WalkBuilder;
use tree_sitter::{Language, Parser};

use crate::grep_filter::build_globset;
use crate::structural::{file_matches_lang, lang_wrapper};
use crate::types::{RewriteFileResult, RewriteInput, RewriteResponse};

// ── Per-path write serialization (issue #47) ──────────────────────────────────
//
// `/rewrite apply=true` does an unsynchronized read→compute→persist. Two
// concurrent calls on the SAME file each compute `modified` from their own
// snapshot and the last persist silently overwrites the other's edit. We
// serialize the whole critical section per canonicalized target path via a
// fixed-size stripe of mutexes keyed by a hash of the canonical path: same
// path → same stripe → serialized; different files usually hash to different
// stripes and proceed in parallel. No new dependency (std::sync only).
const WRITE_LOCK_STRIPES: usize = 64;

fn write_locks() -> &'static [Mutex<()>] {
    static LOCKS: OnceLock<Vec<Mutex<()>>> = OnceLock::new();
    LOCKS.get_or_init(|| (0..WRITE_LOCK_STRIPES).map(|_| Mutex::new(())).collect())
}

/// Pick the stripe mutex for `path`. Canonicalizes so that two paths referring
/// to the same file (after symlink resolution) hash to the same stripe; falls
/// back to the literal path if canonicalize fails (file not yet readable).
///
/// F5 (#53): if canonicalize fails we fall back to the literal path, which can
/// split two concurrent callers for the SAME file onto different stripes (one
/// canonicalized, one literal) — reopening the #47 race for that pair. We
/// warn-log (once) so the fallback is observable rather than silent.
fn stripe_for(path: &Path) -> &'static Mutex<()> {
    let key = match std::fs::canonicalize(path) {
        Ok(c) => c,
        Err(_) => {
            static WARNED: OnceLock<()> = OnceLock::new();
            WARNED.get_or_init(|| {
                tracing::warn!(
                    path = %path.display(),
                    "rewrite: canonicalize failed, falling back to literal path for stripe hashing \
                     (concurrent same-file callers may land on different stripes — issue #47 race)"
                );
            });
            path.to_path_buf()
        }
    };
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    let idx = (h.finish() as usize) % WRITE_LOCK_STRIPES;
    &write_locks()[idx]
}

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
    let exclude = input
        .exclude_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    let root = Path::new(&input.root);
    let mut files: Vec<RewriteFileResult> = Vec::new();
    let mut total_matches = 0usize;
    let mut total_skipped = 0usize;
    let mut rejected: Vec<crate::types::RewriteRejection> = Vec::new();

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

        // For apply=true, serialize the full read→compute→persist critical
        // section per canonicalized path so two concurrent rewrites of the
        // SAME file cannot lose an edit (issue #47). Dry runs hold no lock.
        //
        // F3 (#53): recover from poison deliberately. The stripe Mutex guards
        // NO data (Mutex<()>), so recovery is safe — a panic elsewhere in the
        // guarded span would otherwise poison this stripe FOREVER (1/64 of
        // paths permanently rejected with a sticky DoS). We log the recovery
        // so it's diagnosable rather than silently absorbed.
        let _write_guard = if input.apply {
            Some(stripe_for(path).lock().unwrap_or_else(|e| {
                tracing::warn!(
                    path = %path.display(),
                    "rewrite: stripe write-lock poisoned, recovering (Mutex<()> guards no data)"
                );
                e.into_inner()
            }))
        } else {
            None
        };

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

        // Bug A (#41): ast-grep `find_all` returns nested/overlapping matches
        // (e.g. `foo($$$ARGS)` on `foo(foo(x))` yields an outer + inner match,
        // each replacement captured from the pristine tree). Applying both
        // with reverse-order `replace_range` lets the outer edit overwrite the
        // inner edit's bytes using the untouched original inner snippet,
        // silently erasing the inner rewrite. Resolve overlaps BEFORE applying:
        // sort by (pos asc, widest-first at equal start), greedily accept
        // non-overlapping edits, skip any whose `[pos, pos+del)` intersects an
        // already-accepted one — mirroring the ast-grep CLI's conflicting-edit
        // skip. `total_matches`/`matches` count only APPLIED edits; `skipped`
        // surfaces the dropped count.
        let (accepted, skipped) = resolve_overlaps(edits);
        let applied = accepted.len();

        let modified = apply_edits(&src, accepted);

        if input.apply {
            // Post-write invariant (#41/#53): re-parse the modified buffer with
            // the SAME grammar used for matching. If it fails to parse at all
            // or introduces NEW ERROR/MISSING nodes (F1: detected via the
            // is_error()/is_missing() flags, NOT kind() string compares) that
            // were not in the original tree, do NOT persist that file.
            //
            // F2 (#53): do NOT bail the whole batch — earlier files in walk
            // order are already persisted, and the server maps any Err→400,
            // discarding the result so the caller can't tell which files were
            // already mutated. Instead record the file as REJECTED, leave it
            // untouched, and CONTINUE the walk so valid files still land.
            let ts_lang = wrapper.get_ts_language();
            let orig_errors = count_errors(&src, &ts_lang).unwrap_or(usize::MAX);
            let new_errors = count_errors(&modified, &ts_lang).unwrap_or(usize::MAX);
            if new_errors > orig_errors {
                rejected.push(crate::types::RewriteRejection {
                    file: rel_path.clone(),
                    reason: format!(
                        "re-parse introduced {} new ERROR/MISSING node(s) \
                         (original={}, modified={}); the rewrite would corrupt the file",
                        new_errors - orig_errors,
                        orig_errors,
                        new_errors
                    ),
                });
                continue;
            }

            // Atomic write: NamedTempFile gives unique name + persist() does rename(2).
            // WalkBuilder(follow_links=false) means we only write files inside root.
            let dir = path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("no parent for {}", path.display()))?;
            let mut tmp = tempfile::NamedTempFile::new_in(dir)
                .map_err(|e| anyhow::anyhow!("rewrite: create tmp in {}: {e}", dir.display()))?;
            use std::io::Write as _;
            tmp.write_all(modified.as_bytes())
                .map_err(|e| anyhow::anyhow!("rewrite: write tmp: {e}"))?;
            tmp.persist(path)
                .map_err(|e| anyhow::anyhow!("rewrite: persist {}: {e}", path.display()))?;
        }

        let diff = unified_diff(&rel_path, &src, &modified);

        // Re-review (#53): count only PERSISTED files. Incrementing before the
        // post-write re-parse invariant would (a) over-report total_matches
        // (rejected files inflate it, contradicting the API contract) and
        // (b) let the max_results early-exit below fire on PHANTOM rejected
        // matches — breaking the walk so a valid later file is never processed
        // and lands in neither files[] nor rejected[] (a silent lost rewrite,
        // the exact #41 class). A rejected file `continue`s above and never
        // reaches here, so counting here is correct for both apply and dry-run.
        total_matches += applied;
        total_skipped += skipped;

        files.push(RewriteFileResult {
            file: rel_path,
            matches: applied,
            skipped,
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
        total_skipped,
        total_files,
        duration_ms: start.elapsed().as_millis() as u64,
        rejected,
    })
}

/// Apply a list of (position, deleted_length, replacement) edits to source.
/// Edits MUST be non-overlapping (caller runs them through `resolve_overlaps`
/// first); they are applied in reverse position order so earlier edits don't
/// shift the byte offsets of later ones.
fn apply_edits(source: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|(pos, _, _)| *pos);
    let mut result = source.to_string();
    for (pos, del_len, replacement) in edits.into_iter().rev() {
        let end = (pos + del_len).min(result.len());
        result.replace_range(pos..end, &replacement);
    }
    result
}

/// Resolve overlapping/nested edits before applying them.
///
/// Sort by `pos` ascending; at equal start, widest (longest deletion) first so
/// the outer of two nested matches deterministically wins over the inner one.
/// Then greedily accept edits whose `[pos, pos+del)` range does NOT intersect
/// the tail (`last_end`) of the already-accepted run, and skip the rest.
/// Adjacent (touching, `pos == last_end`) edits are NOT overlapping and are
/// both accepted. Mirrors the ast-grep CLI's conflicting-edit skip.
///
/// Returns `(accepted, skipped_count)`.
fn resolve_overlaps(
    mut edits: Vec<(usize, usize, String)>,
) -> (Vec<(usize, usize, String)>, usize) {
    edits.sort_by(|(p1, d1, _), (p2, d2, _)| p1.cmp(p2).then_with(|| d2.cmp(d1)));
    let mut accepted: Vec<(usize, usize, String)> = Vec::with_capacity(edits.len());
    let mut last_end: Option<usize> = None;
    let mut skipped = 0usize;
    for (pos, del, repl) in edits {
        let end = pos + del;
        if last_end.is_some_and(|le| pos < le) {
            // This edit's start lies inside a previously accepted edit's range.
            skipped += 1;
            continue;
        }
        accepted.push((pos, del, repl));
        last_end = Some(end);
    }
    (accepted, skipped)
}

/// Parse `src` with `lang` and count `ERROR`/`MISSING` nodes in the tree.
/// Returns `None` if the buffer fails to parse at all (treated as maximally
/// erroneous by the caller's invariant check).
fn count_errors(src: &str, lang: &Language) -> Option<usize> {
    let mut parser = Parser::new();
    parser.set_language(lang).ok()?;
    let tree = parser.parse(src.as_bytes(), None)?;
    Some(count_errors_in(tree.root_node()))
}

fn count_errors_in(node: tree_sitter::Node) -> usize {
    let mut n = 0;
    // F1 (#53): use the `is_error()`/`is_missing()` flags, NOT kind() string
    // compares. A MISSING (zero-width, auto-inserted) node's `kind()` is the
    // EXPECTED grammar symbol (e.g. `")"`, `"}"`), never the literal
    // `"MISSING"` — so `kind() == "MISSING"` never fired and a rewrite that
    // drops a required closing/terminator token (recovered via a pure MISSING
    // insertion with NO ERROR node) silently passed the invariant.
    if node.is_error() || node.is_missing() {
        n += 1;
    }
    let mut i = 0;
    while let Some(child) = node.child(i) {
        n += count_errors_in(child);
        i += 1;
    }
    n
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
    use std::sync::Arc;
    use std::thread;
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
        fs::write(
            dir.path().join("main.go"),
            "package main\n\nfunc main() {}\n",
        )
        .unwrap();
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
        let edits = vec![(0, 3, "YYY".to_string()), (4, 3, "XXX".to_string())];
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
        fs::write(
            &file_path,
            "package main\nfunc f() {\nif err != nil { return err }\n}\n",
        )
        .unwrap();
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
        assert!(
            content.contains("fmt.Errorf"),
            "file not updated on disk: {}",
            content
        );
    }

    /// Re-parse `src` with the tree-sitter grammar for `lang_name` and return
    /// the number of ERROR/MISSING nodes in the tree. Used to assert the
    /// persisted buffer is not silently corrupted into invalid syntax.
    ///
    /// F7 (#53): delegates to the production `count_errors_in` (via the
    /// `count_errors` helper) instead of re-implementing the tree walk — the
    /// only part that legitimately differs is the lang-resolution/parse
    /// boilerplate (tests resolve via `ox_langs::get_language`, production via
    /// the ast-grep `LangWrapper`).
    fn count_error_nodes(src: &str, lang_name: &str) -> usize {
        let cfg = match ox_langs::get_language(lang_name) {
            Some(c) => c,
            None => return 0,
        };
        count_errors(src, &cfg.language).unwrap_or(usize::MAX)
    }

    /// Bug A (#41): nested/overlapping ast-grep matches must not silently
    /// corrupt the file. `foo($$$ARGS)` matches `foo(foo(x))` at BOTH nesting
    /// depths (outer + inner); the outer edit's replacement is captured from
    /// the pristine tree and would overwrite the inner edit's bytes, erasing
    /// the inner rewrite while still counting it in `total_matches`.
    ///
    /// Fix: detect overlapping byte ranges, greedily accept non-overlapping
    /// edits (widest-first at equal start), skip the rest, surface the skipped
    /// count, and re-parse the result with the same grammar before persisting.
    #[test]
    fn test_rewrite_nested_overlapping_no_corruption() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.js");
        // `foo(foo(x))` — outer match = whole call, inner match = `foo(x)`.
        fs::write(&file_path, "foo(foo(x))\n").unwrap();

        let input = RewriteInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "foo($$$ARGS)".into(),
            rewrite: "bar($$$ARGS)".into(),
            language: "javascript".into(),
            max_results: 50,
            file_glob: None,
            exclude_glob: None,
            apply: true,
        };
        let result = rewrite(input).unwrap();
        let f = &result.files[0];

        // (b) overlapping matches handled deterministically + skipped reported.
        // Today: find_all returns 2 nested matches, both counted, inner erased.
        // After fix: only the outer (widest, first) is applied; inner is skipped.
        assert_eq!(
            f.matches,
            1,
            "matches must count only APPLIED edits, got {} (raw find_all={})",
            f.matches,
            f.matches + f.skipped
        );
        assert_eq!(
            f.skipped, 1,
            "the nested inner match must be reported as skipped, not silently dropped"
        );
        assert_eq!(result.total_matches, 1);
        assert_eq!(result.total_skipped, 1);

        // (a) NO silent corruption: the persisted buffer re-parses cleanly with
        // the SAME grammar (no new ERROR/MISSING nodes).
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            count_error_nodes(&content, "javascript"),
            0,
            "persisted buffer does not re-parse cleanly: {content}"
        );
        // Deterministic outcome: outer applied, inner skipped.
        assert_eq!(content, "bar(foo(x))\n");
    }

    /// Bug A (#41) — post-write re-parse invariant: a rewrite whose result is
    /// syntactically invalid for the target grammar must NOT be persisted.
    /// After F2 (#53) the file is recorded in `rejected` (not a batch-killing
    /// Err) and the file is left untouched on disk.
    #[test]
    fn test_rewrite_apply_reparse_invariant_rejects_invalid() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.js");
        let original = "foo(x)\n";
        fs::write(&file_path, original).unwrap();

        let input = RewriteInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "foo($$$ARGS)".into(),
            // Replacement is syntactically invalid JavaScript.
            rewrite: ")))(((".into(),
            language: "javascript".into(),
            max_results: 50,
            file_glob: None,
            exclude_glob: None,
            apply: true,
        };
        let result =
            rewrite(input).expect("batch must not Err on a per-file invariant failure (F2)");

        // File must be left untouched (not persisted).
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            content, original,
            "corrupt buffer must not be persisted; file was changed to: {content}"
        );
        // The bad file must be reported in `rejected`, not in `files`.
        assert_eq!(
            result.files.len(),
            0,
            "rejected file must not appear in files"
        );
        assert_eq!(result.rejected.len(), 1, "bad file must be in rejected");
        assert!(result.rejected[0].file.contains("test.js"));
    }

    /// F1 (#53): MISSING-only recovery must be detected by the re-parse
    /// invariant. tree-sitter represents a missing (zero-width, auto-inserted)
    /// token as a node whose `kind()` is the EXPECTED grammar symbol (e.g. `")"`,
    /// `"}"`), NOT the literal string `"MISSING"` — and whose `is_missing()`
    /// flag is set. The old guard checked `node.kind() == "MISSING"` (never
    /// true), so a rewrite that drops a required closing token — recovered by
    /// tree-sitter via a pure MISSING insertion with NO ERROR node — silently
    /// passed the invariant and got persisted. This rewrite removes the closing
    /// `}` of a Go function body, producing a MISSING `}` with no ERROR node.
    #[test]
    fn test_rewrite_apply_reparse_invariant_rejects_missing_only_recovery() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("main.go");
        let original = "package main\nfunc f() { x := 1 }\n";
        fs::write(&file_path, original).unwrap();

        let input = RewriteInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "func $N() { $$$B }".into(),
            // Drop the closing `}` — tree-sitter-go recovers via a MISSING `}`
            // insertion with no ERROR node.
            rewrite: "func $N() { $$$B".into(),
            language: "go".into(),
            max_results: 50,
            file_glob: None,
            exclude_glob: None,
            apply: true,
        };
        let result = rewrite(input);

        // The file must NOT be persisted (rejected via Err or recorded in
        // `rejected`). Either way, the on-disk content is untouched.
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            content, original,
            "MISSING-only corruption must not be persisted; file was changed to: {content}"
        );
        // And the rewrite must surface the rejection (not silently Ok with
        // the bad file counted in `files` as a successful write).
        if let Ok(ref r) = result {
            assert!(
                !r.rejected.is_empty(),
                "MISSING-only corruption must be reported in `rejected`, \
                 got Ok with no rejections: {:?}",
                r
            );
            assert!(
                r.files.is_empty(),
                "rejected file must not also appear in `files`: {:?}",
                r.files
            );
        }
        // Err is also acceptable (hard error, file is untouched).
    }

    /// Bug B (#47): two concurrent `/rewrite apply=true` on the SAME file but
    /// different non-overlapping match sites must both land. Today the
    /// read-compute-persist is unsynchronized, so the last persist silently
    /// overwrites the other's edit. After fix a per-canonical-path lock
    /// serializes the write path.
    #[test]
    fn test_rewrite_concurrent_apply_same_file() {
        let dir = TempDir::new().unwrap();
        let dir = Arc::new(dir.path().to_path_buf());
        let file_path = dir.join("test.js");
        fs::write(&file_path, "foo(x)\nbar(y)\n").unwrap();

        let dir1 = dir.clone();
        let t1 = thread::spawn(move || {
            let input = RewriteInput {
                root: dir1.to_string_lossy().into(),
                pattern: "foo($$$A)".into(),
                rewrite: "foo2($$$A)".into(),
                language: "javascript".into(),
                max_results: 50,
                file_glob: None,
                exclude_glob: None,
                apply: true,
            };
            rewrite(input).unwrap()
        });

        let dir2 = dir.clone();
        let t2 = thread::spawn(move || {
            let input = RewriteInput {
                root: dir2.to_string_lossy().into(),
                pattern: "bar($$$A)".into(),
                rewrite: "bar2($$$A)".into(),
                language: "javascript".into(),
                max_results: 50,
                file_glob: None,
                exclude_glob: None,
                apply: true,
            };
            rewrite(input).unwrap()
        });

        let r1 = t1.join().unwrap();
        let r2 = t2.join().unwrap();
        assert!(r1.total_matches >= 1 && r2.total_matches >= 1);

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(
            content.contains("foo2") && content.contains("bar2"),
            "both concurrent edits must be present; lost an update. file={content}"
        );
    }

    /// F2 (#53): a batch with one invalid-rewrite file must still persist the
    /// valid files and report the bad one in `rejected` — NOT return a 400
    /// (Err) that discards the whole batch. Before F2, the per-file invariant
    /// `bail!` killed the whole `rewrite()` on the first bad file; earlier
    /// files in walk order were already persisted but the server mapped Err→400
    /// and dropped the result, so the caller couldn't tell which files were
    /// already mutated.
    ///
    /// Trick: one literal pattern `foo(x)` + one literal rewrite `foo(x))`
    /// (adds an extra `)`). valid.js has a pre-existing MISSING `)` (unclosed
    /// paren), so the extra `)` FIXES it (new_errors < orig_errors →
    /// persisted). bad.js is clean, so the extra `)` CREATES an ERROR
    /// (new_errors > orig_errors → rejected). Both paths in one batch call.
    #[test]
    fn test_rewrite_batch_partial_persists_valid_reports_rejected() {
        let dir = TempDir::new().unwrap();
        // valid.js: `(foo(x)` has a MISSING `)` (1 error in original). The
        // rewrite's extra `)` closes the paren → 0 errors → PERSISTED.
        let valid_path = dir.path().join("valid.js");
        let valid_original = "(foo(x)\n";
        fs::write(&valid_path, valid_original).unwrap();
        // bad.js: `foo(x)` is clean (0 errors). The rewrite's extra `)` →
        // 1 ERROR → REJECTED.
        let bad_path = dir.path().join("bad.js");
        let bad_original = "foo(x)\n";
        fs::write(&bad_path, bad_original).unwrap();

        // One pattern+rewrite: `foo(x)` → `foo(x))` (adds extra `)`).
        let input = RewriteInput {
            root: dir.path().to_string_lossy().into(),
            pattern: "foo(x)".into(),
            rewrite: "foo(x))".into(),
            language: "javascript".into(),
            max_results: 50,
            file_glob: None,
            exclude_glob: None,
            apply: true,
        };
        let result = rewrite(input).expect("batch must NOT Err on a per-file rejection (F2)");

        // valid.js was persisted (the extra `)` fixed the MISSING `)`).
        let valid_content = fs::read_to_string(&valid_path).unwrap();
        assert_eq!(
            valid_content, "(foo(x))\n",
            "valid file must be persisted; got: {valid_content}"
        );
        // bad.js was NOT persisted (left untouched — extra `)` creates ERROR).
        let bad_content = fs::read_to_string(&bad_path).unwrap();
        assert_eq!(
            bad_content, bad_original,
            "bad file must be left untouched; got: {bad_content}"
        );
        // Result reports: 1 file persisted (valid.js), 1 rejected (bad.js).
        assert_eq!(
            result.files.len(),
            1,
            "exactly the valid file must be in files"
        );
        assert!(result.files[0].file.contains("valid.js"));
        assert_eq!(result.rejected.len(), 1, "the bad file must be in rejected");
        assert!(result.rejected[0].file.contains("bad.js"));
        // Re-review (#53): aggregates count only PERSISTED files — the rejected
        // bad.js must NOT inflate total_matches (it would to 2 if counted before
        // the invariant), which is also what keeps the max_results early-exit
        // from breaking the walk on a phantom rejected match (silent-loss #41).
        assert_eq!(
            result.total_matches, 1,
            "total_matches must count only the persisted file, not the rejected one"
        );
        assert_eq!(result.total_skipped, 0);
    }

    /// F6 (#53): `resolve_overlaps` tie-break at equal start position must
    /// keep the WIDEST edit (longest deletion). The existing fixture
    /// `foo(foo(x))` has DIFFERENT start positions so the tie-break never
    /// runs. This test uses two edits at the SAME `pos` with different `del`
    /// lengths and asserts the wider one is kept, the narrower is skipped.
    #[test]
    fn test_resolve_overlaps_same_start_widest_first() {
        // Two edits at pos=5: one deletes 3 bytes (narrow), one deletes 7
        // bytes (wide). The widest-first tie-break must keep the wide one.
        let edits = vec![(5, 3, "NARROW".to_string()), (5, 7, "WIDE".to_string())];
        let (accepted, skipped) = resolve_overlaps(edits);
        assert_eq!(accepted.len(), 1, "only the widest edit must be accepted");
        assert_eq!(
            accepted[0].2, "WIDE",
            "the widest edit must win the tie-break"
        );
        assert_eq!(skipped, 1, "the narrower edit must be reported as skipped");
    }
}
