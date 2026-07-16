use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, UNIX_EPOCH};

use anyhow::Result;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use ox_core::grep_filter::build_globset;
use ox_core::walk::{WalkBudget, filtered_walk};
use ox_dataflow::{DataflowInput, DataflowResponse, Finding, analysis, scope_walker};
use ox_langs::{get_language, language_id};

use crate::dataflow_cache::{DataflowCache, DataflowCacheKey};
use crate::walk_guard::guarded_walk;

/// Server-side maximum for caller-supplied `max_results`. An explicit
/// oversized value (or the default 100) is clamped to this — without it, a
/// caller can request unbounded findings, producing huge cache entries.
const MAX_MAX_RESULTS: usize = 1000;

/// Server-side maximum for caller-supplied `max_files`. An explicit JSON
/// `null` deserializes to `None` (walks everything) — clamped to this. An
/// explicit oversized value is also clamped. Without this, a caller can
/// force a full-repo walk on arbitrarily large repos.
const MAX_MAX_FILES: usize = 10_000;

/// Clamp caller-supplied `max_results` and `max_files` to server-side maximums.
///
/// Without this, an explicit JSON `null` for `max_files` deserializes to `None`
/// (walks everything) and an oversized `max_results` passes through unclamped —
/// only ABSENT fields get a serde default. This produces unbounded cache entries
/// and multi-minute walks on large repos.
fn clamp_input(mut input: DataflowInput) -> DataflowInput {
    input.max_results = input.max_results.min(MAX_MAX_RESULTS);
    input.max_files = Some(input.max_files.unwrap_or(MAX_MAX_FILES).min(MAX_MAX_FILES));
    input
}

pub async fn handle(
    State(state): State<crate::AppState>,
    Json(input): Json<DataflowInput>,
) -> Result<Json<DataflowResponse>, (StatusCode, String)> {
    // Clamp caller-supplied limits to server-side maximums before any analysis
    // or caching — prevents unbounded walks and oversized cache entries.
    let input = clamp_input(input);

    let dataflow_cache = state.dataflow_cache.clone();
    let result = guarded_walk(move || analyze_directory(input, &dataflow_cache)).await?;
    Ok(Json(result))
}

pub(crate) fn analyze_directory(
    input: DataflowInput,
    cache: &DataflowCache,
) -> Result<DataflowResponse> {
    let start = Instant::now();
    let mut input = input;
    let canonical_root = match std::fs::canonicalize(&input.root) {
        Ok(p) => p,
        // Canonicalization failed: preserve the cache-cold path and skip caching.
        Err(_) => return analyze_uncached(input),
    };
    input.root = canonical_root.to_string_lossy().into_owned();

    let key = match build_key(&input, &canonical_root) {
        Ok(k) => k,
        // Key construction failed (e.g. unsupported language or bad glob):
        // fall back to the cache-cold path.
        Err(_) => return analyze_uncached(input),
    };

    let (response, is_hit) =
        cache.get_or_insert(key, move || Ok(Arc::new(analyze_uncached(input)?)))?;
    let mut response = (*response).clone();
    response.is_hit = is_hit;
    if is_hit {
        response.duration_ms = start.elapsed().as_millis() as u64;
    }
    Ok(response)
}

fn build_key(input: &DataflowInput, canonical_root: &Path) -> Result<DataflowCacheKey> {
    let lang_cfg = get_language(&input.language)
        .ok_or_else(|| anyhow::anyhow!("unsupported language: {}", input.language))?;
    // Normalize the language alias to its canonical id (parity with the scope
    // cache) so `go`/`golang` (or `ts`/`typescript`) share one cache entry
    // instead of producing duplicate recompute + double memory.
    let canonical_lang = language_id(&input.language)
        .ok_or_else(|| anyhow::anyhow!("unsupported language: {}", input.language))?;
    let include = input.file_glob.as_deref().map(build_globset).transpose()?;
    let exclude = input
        .exclude_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    let aggregate_fingerprint = compute_fingerprint(
        canonical_root,
        &lang_cfg,
        include.as_ref(),
        exclude.as_ref(),
        input.max_files,
    )?;

    Ok(DataflowCacheKey {
        canonical_root: canonical_root.to_path_buf(),
        language: canonical_lang.to_string(),
        file_glob: input.file_glob.clone(),
        exclude_glob: input.exclude_glob.clone(),
        max_results: input.max_results,
        aggregate_fingerprint,
    })
}

fn compute_fingerprint(
    root: &Path,
    lang_cfg: &ox_langs::LangConfig,
    include: Option<&globset::GlobSet>,
    exclude: Option<&globset::GlobSet>,
    max_files: Option<usize>,
) -> Result<u64> {
    let mut aggregate: u64 = 0;

    let budget = WalkBudget {
        max_files,
        // PR1: no byte-cap (None = no behavior change). PR2 flips this to
        // Some(ox_core::walk::DEFAULT_MAX_FILE_BYTES).
        max_file_bytes: None,
    };
    for (_path, rel, metadata) in
        filtered_walk(root, Some(lang_cfg.extensions), include, exclude, budget)
    {
        let mtime_nanos = metadata
            .modified()
            .unwrap_or(UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let file_len = metadata.len();

        let mut hasher = DefaultHasher::new();
        rel.as_bytes().hash(&mut hasher);
        mtime_nanos.hash(&mut hasher);
        file_len.hash(&mut hasher);
        aggregate ^= hasher.finish();
    }

    Ok(aggregate)
}

fn analyze_uncached(input: DataflowInput) -> Result<DataflowResponse> {
    let start = Instant::now();

    let lang_cfg = get_language(&input.language)
        .ok_or_else(|| anyhow::anyhow!("unsupported language: {}", input.language))?;

    let include = input.file_glob.as_deref().map(build_globset).transpose()?;
    let exclude = input
        .exclude_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    let root = Path::new(&input.root);
    let mut all_findings: Vec<Finding> = Vec::new();
    let mut files_analyzed: usize = 0;

    let budget = WalkBudget {
        max_files: input.max_files,
        // PR1: no byte-cap (None = no behavior change). PR2 flips this to
        // Some(ox_core::walk::DEFAULT_MAX_FILE_BYTES).
        max_file_bytes: None,
    };
    let mut walk = filtered_walk(
        root,
        Some(lang_cfg.extensions),
        include.as_ref(),
        exclude.as_ref(),
        budget,
    );
    let mut broke_early = false;
    for (path, rel, _metadata) in walk.by_ref() {
        let source = match std::fs::read(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let chain = match scope_walker::walk_file_with_ext(&source, &input.language, ext) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let findings = analysis::analyze(&chain, &rel);
        all_findings.extend(findings);
        files_analyzed += 1;

        if all_findings.len() >= input.max_results * 5 {
            broke_early = true;
            break;
        }
    }
    // `walk.truncated()` is the sole source of truth: it is true iff a
    // qualifying file existed beyond position cap (checked after pulling, not
    // before). The old `|| files_analyzed >= cap` disjunct fired at exact-cap
    // even when nothing was skipped — removed.
    let mut files_truncated = walk.truncated();
    // Early-break edge case: if the findings budget (`max_results * 5`) fired
    // at exactly `files_analyzed == cap`, the walk never got a chance to run
    // its cap check (which fires on the NEXT pull, not the current one). So
    // `walk.truncated()` is still false even though a qualifying file exists
    // beyond cap — a real truncation is silently missed. Probe one more pull:
    // if the cap is hit, `FilteredWalk::next` sets `truncated` internally and
    // returns `None`; we re-read `walk.truncated()` to capture it. If the walk
    // naturally exhausts (no more qualifying files), `truncated` stays false —
    // no false positive (parity with the exact-cap boundary fix in #50).
    if broke_early && input.max_files.is_some() && !files_truncated {
        let _ = walk.next();
        files_truncated = walk.truncated();
    }

    let total = all_findings.len();
    let truncated = total > input.max_results;
    if truncated {
        all_findings.truncate(input.max_results);
    }

    Ok(DataflowResponse {
        findings: all_findings,
        total_findings: total,
        files_analyzed,
        truncated,
        files_truncated,
        duration_ms: start.elapsed().as_millis() as u64,
        is_hit: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataflow_cache::DataflowCache;
    use ox_dataflow::DataflowInput;
    use tempfile::tempdir;

    /// clamp_input must cap oversized caller-supplied max_results and convert
    /// explicit null max_files (None → walks everything) to a server-side max.
    /// Reverting clamp_input (passing values through unclamped) REDS this test.
    #[test]
    fn test_clamp_input_caps_oversized_values() {
        let input = DataflowInput {
            root: "/tmp".into(),
            language: "typescript".into(),
            max_results: 100_000,
            max_files: None,
            file_glob: None,
            exclude_glob: None,
        };
        let clamped = clamp_input(input);
        assert_eq!(
            clamped.max_results, MAX_MAX_RESULTS,
            "oversized max_results should be clamped to server-side max"
        );
        assert_eq!(
            clamped.max_files,
            Some(MAX_MAX_FILES),
            "null max_files (None) should be clamped to Some(MAX_MAX_FILES)"
        );
    }

    /// clamp_input must leave already-in-range values unchanged.
    #[test]
    fn test_clamp_input_passes_through_in_range_values() {
        let input = DataflowInput {
            root: "/tmp".into(),
            language: "typescript".into(),
            max_results: 50,
            max_files: Some(500),
            file_glob: None,
            exclude_glob: None,
        };
        let clamped = clamp_input(input);
        assert_eq!(clamped.max_results, 50);
        assert_eq!(clamped.max_files, Some(500));
    }

    /// Verifies that analyze_directory stops at max_files even when the
    /// directory contains more matching files.
    #[test]
    fn test_analyze_directory_respects_max_files() {
        let dir = tempdir().unwrap();
        // Create 10 .ts files.
        for i in 0..10 {
            let path = dir.path().join(format!("file{i}.ts"));
            std::fs::write(&path, b"const x = 1;").unwrap();
        }

        let input = DataflowInput {
            root: dir.path().to_string_lossy().into_owned(),
            language: "typescript".to_string(),
            max_results: 100,
            max_files: Some(3),
            file_glob: None,
            exclude_glob: None,
        };

        let cache = DataflowCache::new();
        let result = analyze_directory(input, &cache).unwrap();
        assert_eq!(
            result.files_analyzed, 3,
            "expected exactly 3 files, got {}",
            result.files_analyzed
        );
        assert!(
            result.files_truncated,
            "files_truncated should be true when cap is hit"
        );
    }

    /// Verifies that a directory with EXACTLY `max_files` qualifying files is NOT
    /// falsely flagged as truncated. `files_truncated` must mean "a qualifying
    /// file existed BEYOND position cap", not "count == cap".
    /// Reverting the cap check to the top of `FilteredWalk::next` (or restoring
    /// the `files_analyzed >= cap` OR-disjunct) REDS this test.
    #[test]
    fn test_analyze_directory_exact_cap_boundary() {
        let dir = tempdir().unwrap();
        // Create exactly 3 .ts files — the cap is also 3.
        for i in 0..3 {
            let path = dir.path().join(format!("file{i}.ts"));
            std::fs::write(&path, b"const x = 1;").unwrap();
        }

        let input = DataflowInput {
            root: dir.path().to_string_lossy().into_owned(),
            language: "typescript".to_string(),
            max_results: 100,
            max_files: Some(3),
            file_glob: None,
            exclude_glob: None,
        };

        let cache = DataflowCache::new();
        let result = analyze_directory(input, &cache).unwrap();
        assert_eq!(result.files_analyzed, 3, "all 3 files should be analyzed");
        assert!(
            !result.files_truncated,
            "files_truncated must be false when the walk naturally exhausted at exactly the cap \
             (no qualifying file was skipped)"
        );
    }

    /// Verifies that a findings-budget early-break at exactly `files_analyzed
    /// == cap` still reports `files_truncated = true` when more qualifying
    /// files exist beyond cap. Without the probe in `analyze_uncached`, the
    /// walk's cap check never fires (it runs on the NEXT pull, which we
    /// skipped by breaking) and `walk.truncated()` is false — a real
    /// truncation is silently missed.
    /// Reverting the `broke_early` probe (deleting the `walk.next()` call)
    /// REDS this test: `files_truncated` reverts to false.
    #[test]
    fn test_analyze_directory_early_break_at_cap_reports_truncated() {
        let dir = tempdir().unwrap();
        // 10 .go files, each producing >=2 findings (unused_var + const_value).
        // With max_results=1, the findings budget is 1*5=5. After 3 files
        // (each producing >=2 findings → >=6 total), the early-break fires at
        // exactly files_analyzed == 3 == cap. 7 more qualifying files exist
        // beyond cap.
        for i in 0..10 {
            let path = dir.path().join(format!("file{i}.go"));
            std::fs::write(&path, "package main\nfunc f() { x := 1 }\n").unwrap();
        }

        let input = DataflowInput {
            root: dir.path().to_string_lossy().into_owned(),
            language: "go".to_string(),
            max_results: 1,
            max_files: Some(3),
            file_glob: None,
            exclude_glob: None,
        };

        let cache = DataflowCache::new();
        let result = analyze_directory(input, &cache).unwrap();
        // The early-break should fire at or before 3 files (the cap).
        assert!(
            result.files_analyzed <= 3,
            "early-break should fire by cap=3, got {}",
            result.files_analyzed
        );
        assert!(
            result.files_truncated,
            "files_truncated must be true when early-break fires at cap with more \
             qualifying files beyond cap; got files_analyzed={}",
            result.files_analyzed
        );
    }

    /// Verifies that max_files=None disables the cap (all files walked).
    #[test]
    fn test_analyze_directory_no_cap() {
        let dir = tempdir().unwrap();
        for i in 0..5 {
            let path = dir.path().join(format!("file{i}.ts"));
            std::fs::write(&path, b"const x = 1;").unwrap();
        }

        let input = DataflowInput {
            root: dir.path().to_string_lossy().into_owned(),
            language: "typescript".to_string(),
            max_results: 100,
            max_files: None,
            file_glob: None,
            exclude_glob: None,
        };

        let cache = DataflowCache::new();
        let result = analyze_directory(input, &cache).unwrap();
        assert_eq!(result.files_analyzed, 5);
    }

    /// Verifies that .svelte files are NOT silently skipped — files_analyzed >= 1.
    #[test]
    fn test_analyze_svelte_not_skipped() {
        let dir = tempdir().unwrap();
        let svelte_src = br#"<script lang="ts">
function leaks() {
    let x = secret();
    return x;
}
</script>
<div>{x}</div>"#;
        let path = dir.path().join("component.svelte");
        std::fs::write(&path, svelte_src).unwrap();

        let input = DataflowInput {
            root: dir.path().to_string_lossy().into_owned(),
            language: "svelte".to_string(),
            max_results: 100,
            max_files: None,
            file_glob: None,
            exclude_glob: None,
        };

        let cache = DataflowCache::new();
        let result = analyze_directory(input, &cache).unwrap();
        assert!(
            result.files_analyzed >= 1,
            "svelte file should not be skipped; files_analyzed={}",
            result.files_analyzed
        );
    }

    /// Regression test for the cap-counting divergence between fingerprint and
    /// analysis. An unreadable middle file must still count toward the max_files
    /// cap so the shared filtered walk cuts both callers at the same file.
    #[cfg(unix)]
    #[test]
    fn test_filtered_walk_counts_read_failures_toward_cap() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let f1 = dir.path().join("file1.ts");
        let f2 = dir.path().join("file2.ts");
        let f3 = dir.path().join("file3.ts");
        std::fs::write(&f1, b"const x = 1;").unwrap();
        std::fs::write(&f2, b"const y = 2;").unwrap();
        std::fs::write(&f3, b"const z = 3;").unwrap();

        std::fs::set_permissions(&f2, std::fs::Permissions::from_mode(0o000)).unwrap();

        // If reading the file still succeeds, the test is running in an
        // environment (e.g. as root) where permissions cannot be dropped.
        if std::fs::read(&f2).is_ok() {
            std::fs::set_permissions(&f2, std::fs::Permissions::from_mode(0o644)).unwrap();
            return;
        }

        let lang_cfg = get_language("typescript").unwrap();
        let root = dir.path();
        let mut walk = filtered_walk(
            root,
            Some(lang_cfg.extensions),
            None,
            None,
            WalkBudget {
                max_files: Some(2),
                max_file_bytes: None,
            },
        );
        let paths: Vec<_> = walk.by_ref().map(|(_, rel, _)| rel).collect();
        let truncated = walk.truncated();
        assert_eq!(paths, vec!["file1.ts", "file2.ts"]);
        assert!(
            truncated,
            "shared walk should truncate at the cap after file2"
        );

        let input = DataflowInput {
            root: root.to_string_lossy().into_owned(),
            language: "typescript".to_string(),
            max_results: 100,
            max_files: Some(2),
            file_glob: None,
            exclude_glob: None,
        };
        let cache = DataflowCache::new();
        let result = analyze_directory(input, &cache).unwrap();
        assert_eq!(
            result.files_analyzed, 1,
            "only file1 should be analyzed; file2 is unreadable and consumes the second cap slot"
        );
        assert!(
            result.files_truncated,
            "analysis should be truncated at the cap after file2"
        );

        std::fs::set_permissions(&f2, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// Verifies the cache hit contract: first call is a miss + one analysis,
    /// second call on the same repo is a hit + zero additional analyses.
    #[test]
    fn test_dataflow_cache_hit_contract() {
        let dir = tempdir().unwrap();
        for i in 0..3 {
            let path = dir.path().join(format!("file{i}.ts"));
            std::fs::write(&path, b"const x = 1;").unwrap();
        }

        let input = DataflowInput {
            root: dir.path().to_string_lossy().into_owned(),
            language: "typescript".to_string(),
            max_results: 100,
            max_files: None,
            file_glob: None,
            exclude_glob: None,
        };

        let cache = DataflowCache::with_capacity(64 * 1024 * 1024);

        let r1 = analyze_directory(input.clone(), &cache).unwrap();
        let (h1, m1, a1) = cache.stats();
        assert_eq!(m1, 1, "first call should be a miss");
        assert_eq!(a1, 1, "first call should run one analysis");
        assert_eq!(h1, 0, "first call should have no hits");

        let r2 = analyze_directory(input, &cache).unwrap();
        let (h2, m2, a2) = cache.stats();
        assert_eq!(h2, 1, "second call should be a hit");
        assert_eq!(m2, 1, "second call should have no additional misses");
        assert_eq!(a2, 1, "second call should run no additional analyses");
        assert!(
            r2.duration_ms < 100,
            "hit path should report a small duration_ms, got {}",
            r2.duration_ms
        );

        let mut r1 = r1;
        r1.duration_ms = 0;
        r1.is_hit = false;
        let mut r2 = r2;
        r2.duration_ms = 0;
        r2.is_hit = false;
        assert_eq!(
            serde_json::to_value(&r1).unwrap(),
            serde_json::to_value(&r2).unwrap(),
            "warm result must equal cold result (modulo is_hit + duration_ms)"
        );
    }

    /// Verifies the `is_hit` observability signal (#48): a first/fresh call
    /// reports `is_hit == false` (miss), and a second call on the same repo
    /// reports `is_hit == true` (cache hit). Reverting the `is_hit` wiring
    /// (always false) REDS this test: the second-call assertion fails.
    #[test]
    fn test_dataflow_response_is_hit_signal() {
        let dir = tempdir().unwrap();
        for i in 0..3 {
            let path = dir.path().join(format!("file{i}.ts"));
            std::fs::write(&path, b"const x = 1;").unwrap();
        }

        let input = DataflowInput {
            root: dir.path().to_string_lossy().into_owned(),
            language: "typescript".to_string(),
            max_results: 100,
            max_files: None,
            file_glob: None,
            exclude_glob: None,
        };

        let cache = DataflowCache::with_capacity(64 * 1024 * 1024);

        let r1 = analyze_directory(input.clone(), &cache).unwrap();
        assert!(
            !r1.is_hit,
            "first call (miss) must report is_hit=false, got {}",
            r1.is_hit
        );

        let r2 = analyze_directory(input, &cache).unwrap();
        assert!(
            r2.is_hit,
            "second call (hit) must report is_hit=true, got {}",
            r2.is_hit
        );
    }

    /// Verifies cache invalidation: modifying a file changes the aggregate
    /// fingerprint and causes a re-analysis.
    #[test]
    fn test_dataflow_cache_invalidation() {
        let dir = tempdir().unwrap();
        for i in 0..3 {
            let path = dir.path().join(format!("file{i}.ts"));
            std::fs::write(&path, b"const x = 1;").unwrap();
        }

        let input = DataflowInput {
            root: dir.path().to_string_lossy().into_owned(),
            language: "typescript".to_string(),
            max_results: 100,
            max_files: None,
            file_glob: None,
            exclude_glob: None,
        };

        let cache = DataflowCache::with_capacity(64 * 1024 * 1024);

        let r1 = analyze_directory(input.clone(), &cache).unwrap();
        let (_h1, m1, a1) = cache.stats();
        assert_eq!(m1, 1, "first call should be a miss");
        assert_eq!(a1, 1);

        let r2 = analyze_directory(input.clone(), &cache).unwrap();
        let (h2, m2, a2) = cache.stats();
        assert_eq!(h2, 1, "second call should be a hit");
        assert_eq!(m2, 1);
        assert_eq!(a2, 1);

        let mut r1 = r1;
        r1.duration_ms = 0;
        r1.is_hit = false;
        let mut r2 = r2;
        r2.duration_ms = 0;
        r2.is_hit = false;
        assert_eq!(
            serde_json::to_value(&r1).unwrap(),
            serde_json::to_value(&r2).unwrap()
        );

        // Append a byte to one file, changing mtime + len.
        let a_path = dir.path().join("file0.ts");
        let mut content = std::fs::read_to_string(&a_path).unwrap();
        content.push_str("\nconst y = 2;\n");
        std::fs::write(&a_path, content).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));

        let r3 = analyze_directory(input, &cache).unwrap();
        let (h3, m3, a3) = cache.stats();
        assert_eq!(m3, 2, "modified file should be a miss");
        assert_eq!(h3, 1, "previous calls should still be hits");
        assert_eq!(a3, 2, "modified file should trigger re-analysis");

        let mut r3 = r3;
        r3.duration_ms = 0;
        r3.is_hit = false;
        assert_ne!(
            serde_json::to_value(&r2).unwrap(),
            serde_json::to_value(&r3).unwrap(),
            "modified repo should produce a different result"
        );
    }

    /// Verifies that a cached response is byte-identical to the uncached one.
    #[test]
    fn test_dataflow_cache_result_equivalence() {
        let dir = tempdir().unwrap();
        for i in 0..3 {
            let path = dir.path().join(format!("file{i}.ts"));
            std::fs::write(&path, b"const x = 1;").unwrap();
        }

        let input = DataflowInput {
            root: dir.path().to_string_lossy().into_owned(),
            language: "typescript".to_string(),
            max_results: 100,
            max_files: None,
            file_glob: None,
            exclude_glob: None,
        };

        let cache = DataflowCache::with_capacity(64 * 1024 * 1024);

        let cold = analyze_directory(input.clone(), &DataflowCache::new()).unwrap();
        let warm1 = analyze_directory(input.clone(), &cache).unwrap();
        let warm2 = analyze_directory(input, &cache).unwrap();

        let mut cold = cold;
        cold.duration_ms = 0;
        cold.is_hit = false;
        let mut warm1 = warm1;
        warm1.duration_ms = 0;
        warm1.is_hit = false;
        let mut warm2 = warm2;
        warm2.duration_ms = 0;
        warm2.is_hit = false;
        assert_eq!(
            serde_json::to_value(&cold).unwrap(),
            serde_json::to_value(&warm1).unwrap(),
            "first warm result must equal cold result"
        );
        assert_eq!(
            serde_json::to_value(&warm1).unwrap(),
            serde_json::to_value(&warm2).unwrap(),
            "second warm result must equal the cached one"
        );

        let (hits, misses, analyses) = cache.stats();
        assert_eq!(misses, 1, "shared cache should have exactly one miss");
        assert_eq!(hits, 1, "second shared-cache call should be a hit");
        assert_eq!(analyses, 1, "only one analysis should run");
    }

    /// Parity with the scope-cache alias test: `go` and `golang` are the same
    /// grammar, so the dataflow cache key must normalize to the canonical id
    /// and share one entry — no duplicate analysis, no double memory.
    #[test]
    fn test_dataflow_cache_alias_key() {
        let dir = tempdir().unwrap();
        for i in 0..3 {
            let path = dir.path().join(format!("file{i}.go"));
            std::fs::write(&path, "package main\n\nfunc F() {}\n").unwrap();
        }

        let root = dir.path().to_string_lossy().into_owned();
        let input = DataflowInput {
            root: root.clone(),
            language: "go".to_string(),
            max_results: 100,
            max_files: None,
            file_glob: None,
            exclude_glob: None,
        };

        let cache = DataflowCache::with_capacity(64 * 1024 * 1024);

        let _ = analyze_directory(input.clone(), &cache).unwrap();
        let (_, m1, a1) = cache.stats();
        assert_eq!(m1, 1, "first call with canonical 'go' should be a miss");
        assert_eq!(a1, 1, "first call should run one analysis");

        let alias_input = DataflowInput {
            language: "golang".to_string(),
            ..input
        };
        let _ = analyze_directory(alias_input, &cache).unwrap();
        let (h2, m2, a2) = cache.stats();
        assert_eq!(m2, 1, "alias 'golang' should not create an additional miss");
        assert_eq!(h2, 1, "alias 'golang' should hit the canonical 'go' entry");
        assert_eq!(a2, 1, "alias 'golang' should not trigger a second analysis");
    }
}
