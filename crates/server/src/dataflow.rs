use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use globset::GlobSet;
use ignore::WalkBuilder;
use tokio::sync::Semaphore;

use ox_core::grep_filter::build_globset;
use ox_dataflow::{DataflowInput, DataflowResponse, Finding, analysis, scope_walker};
use ox_langs::{get_language, language_id};

use crate::dataflow_cache::{DataflowCache, DataflowCacheKey};

/// Hard deadline for a single dataflow request. Note: `spawn_blocking` cannot
/// be cancelled — the task continues running in the blocking thread pool after
/// the timeout fires, but the HTTP response is returned immediately.
const DATAFLOW_TIMEOUT_SECS: u64 = 25;

/// Maximum concurrent directory walks. Each walk can be CPU/IO-heavy; capping
/// at 8 (2x the 4 ARM cores) prevents phantom tasks from saturating the Tokio
/// blocking pool (default 512 threads) during bursts of oversized-repo requests.
/// The permit is held until the blocking task finishes (even after a timeout),
/// so the pool is always bounded to this many active walks.
const SEMAPHORE_PERMITS: usize = 8;

/// Bounded timeout for acquiring a walk permit. If all permits are held by
/// in-flight (potentially stuck) walks, the caller fails fast with HTTP 503
/// instead of queueing forever in `.acquire().await` with no timeout.
const WALK_ACQUIRE_TIMEOUT_SECS: u64 = 5;

/// Server-side maximum for caller-supplied `max_results`. An explicit
/// oversized value (or the default 100) is clamped to this — without it, a
/// caller can request unbounded findings, producing huge cache entries.
const MAX_MAX_RESULTS: usize = 1000;

/// Server-side maximum for caller-supplied `max_files`. An explicit JSON
/// `null` deserializes to `None` (walks everything) — clamped to this. An
/// explicit oversized value is also clamped. Without this, a caller can
/// force a full-repo walk on arbitrarily large repos.
const MAX_MAX_FILES: usize = 10_000;

static WALK_SEMAPHORE: Semaphore = Semaphore::const_new(SEMAPHORE_PERMITS);

// ── Walk observability ───────────────────────────────────────────────────
//
// `WALK_METRICS` tracks how many walks are in-flight and the start timestamp
// of the oldest one. A walk whose in-flight duration exceeds
// `DATAFLOW_TIMEOUT_SECS` is "stuck" — its permit will never be returned
// because `spawn_blocking` cannot be cancelled. The staleness signal lets an
// operator detect a degraded pool (fewer and fewer available permits) before
// the pool is fully exhausted and every request starts getting 503.
//
// Approximation: `oldest_start_ms` is set when the first walk starts and
// cleared when the last walk finishes. If the first walk finishes but others
// remain, `oldest_start_ms` may hold a stale (finished) walk's timestamp,
// producing a false-positive staleness signal. This is acceptable for an
// observability signal — the operator investigates, not auto-scales.

static WALK_METRICS: WalkMetrics = WalkMetrics::new_const();

pub(crate) struct WalkMetrics {
    in_flight: AtomicU64,
    oldest_start_ms: AtomicU64,
}

impl WalkMetrics {
    const fn new_const() -> Self {
        Self {
            in_flight: AtomicU64::new(0),
            oldest_start_ms: AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    fn new() -> Self {
        Self {
            in_flight: AtomicU64::new(0),
            oldest_start_ms: AtomicU64::new(0),
        }
    }

    fn acquire_slot(&self) -> WalkSlot<'_> {
        let now = now_ms();
        let prev = self.in_flight.fetch_add(1, Ordering::Relaxed);
        if prev == 0 {
            self.oldest_start_ms.store(now, Ordering::Relaxed);
        }
        WalkSlot { metrics: self }
    }

    fn stats(&self) -> (u64, u64) {
        (
            self.in_flight.load(Ordering::Relaxed),
            self.oldest_start_ms.load(Ordering::Relaxed),
        )
    }
}

/// RAII guard that increments `in_flight` on creation and decrements on drop.
/// Moved into the `spawn_blocking` closure so it is dropped when the walk
/// finishes (even on panic), not when the outer timeout fires.
pub(crate) struct WalkSlot<'a> {
    metrics: &'a WalkMetrics,
}

impl Drop for WalkSlot<'_> {
    fn drop(&mut self) {
        let prev = self.metrics.in_flight.fetch_sub(1, Ordering::Relaxed);
        if prev == 1 {
            self.metrics.oldest_start_ms.store(0, Ordering::Relaxed);
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Return (in_flight, oldest_start_ms) for the global walk metrics.
/// Exposed via `GET /cache/stats` → `walks` field.
pub(crate) fn walk_metrics() -> (u64, u64) {
    WALK_METRICS.stats()
}

/// Acquire a walk permit with a bounded timeout. On timeout, fail fast with
/// HTTP 503 (backpressure) instead of queueing forever.
async fn acquire_walk_permit(
    semaphore: &Semaphore,
    timeout: Duration,
) -> Result<tokio::sync::SemaphorePermit<'_>, (StatusCode, String)> {
    tokio::time::timeout(timeout, semaphore.acquire())
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "dataflow walk pool saturated; retry later".to_string(),
            )
        })?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

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

    // Acquire a walk permit with a bounded timeout. If all permits are held
    // by in-flight walks, fail fast with 503 instead of queueing forever.
    let _permit = acquire_walk_permit(
        &WALK_SEMAPHORE,
        Duration::from_secs(WALK_ACQUIRE_TIMEOUT_SECS),
    )
    .await?;

    // Track the walk for staleness observability. The slot is moved into the
    // spawn_blocking closure so it is dropped when the walk finishes (even on
    // panic or timeout — the phantom task eventually completes and drops it).
    let slot = WALK_METRICS.acquire_slot();

    let dataflow_cache = state.dataflow_cache.clone();
    let task = tokio::task::spawn_blocking(move || {
        // Keep the permit and slot alive until the blocking task finishes.
        // spawn_blocking cannot be cancelled, so even after the outer timeout
        // fires, this closure runs to completion and then drops both.
        let _slot = slot;
        let _permit = _permit;
        analyze_directory(input, &dataflow_cache)
    });

    let result = tokio::time::timeout(Duration::from_secs(DATAFLOW_TIMEOUT_SECS), task)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "dataflow analysis exceeded time limit".to_string(),
            )
        })?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(result))
}

fn analyze_directory(input: DataflowInput, cache: &DataflowCache) -> Result<DataflowResponse> {
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

struct FilteredWalk<'a> {
    walk: ignore::Walk,
    root: &'a Path,
    lang_cfg: &'a ox_langs::LangConfig,
    include: Option<&'a GlobSet>,
    exclude: Option<&'a GlobSet>,
    max_files: Option<usize>,
    count: usize,
    truncated: bool,
}

impl<'a> FilteredWalk<'a> {
    fn truncated(&self) -> bool {
        self.truncated
    }
}

impl Iterator for FilteredWalk<'_> {
    type Item = (PathBuf, String, std::fs::Metadata);

    fn next(&mut self) -> Option<Self::Item> {
        for result in self.walk.by_ref() {
            let entry = result.ok()?;
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }

            let path = entry.path().to_path_buf();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !self.lang_cfg.extensions.contains(&ext) {
                continue;
            }

            let rel_path = path.strip_prefix(self.root).unwrap_or(&path);
            if let Some(inc) = self.include
                && !inc.is_match(rel_path)
            {
                continue;
            }
            if let Some(exc) = self.exclude
                && exc.is_match(rel_path)
            {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Cap check AFTER pulling a qualifying item, not before. This
            // distinguishes "walk naturally exhausted at exactly cap" (not
            // truncated) from "a qualifying file existed beyond position cap"
            // (truncated). Checking at the top would set truncated=true even
            // when nothing was skipped (e.g. a repo with exactly max_files
            // qualifying files).
            if self.max_files.is_some_and(|cap| self.count >= cap) {
                self.truncated = true;
                return None;
            }

            self.count += 1;
            let rel_str = rel_path.to_string_lossy().into_owned();
            return Some((path, rel_str, metadata));
        }

        None
    }
}

fn filtered_walk<'a>(
    root: &'a Path,
    lang_cfg: &'a ox_langs::LangConfig,
    include: Option<&'a GlobSet>,
    exclude: Option<&'a GlobSet>,
    max_files: Option<usize>,
) -> FilteredWalk<'a> {
    FilteredWalk {
        walk: WalkBuilder::new(root)
            .standard_filters(true)
            .sort_by_file_path(|a, b| a.cmp(b))
            .build(),
        root,
        lang_cfg,
        include,
        exclude,
        max_files,
        count: 0,
        truncated: false,
    }
}

fn compute_fingerprint(
    root: &Path,
    lang_cfg: &ox_langs::LangConfig,
    include: Option<&GlobSet>,
    exclude: Option<&GlobSet>,
    max_files: Option<usize>,
) -> Result<u64> {
    let mut aggregate: u64 = 0;

    for (_path, rel, metadata) in filtered_walk(root, lang_cfg, include, exclude, max_files) {
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

    let mut walk = filtered_walk(
        root,
        &lang_cfg,
        include.as_ref(),
        exclude.as_ref(),
        input.max_files,
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
        let mut walk = filtered_walk(root, &lang_cfg, None, None, Some(2));
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
        let mut r2 = r2;
        r2.duration_ms = 0;
        assert_eq!(
            serde_json::to_value(&r1).unwrap(),
            serde_json::to_value(&r2).unwrap(),
            "warm result must equal cold result"
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
        let mut r2 = r2;
        r2.duration_ms = 0;
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
        let mut warm1 = warm1;
        warm1.duration_ms = 0;
        let mut warm2 = warm2;
        warm2.duration_ms = 0;
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

#[cfg(test)]
mod semaphore_tests {
    use super::*;
    use crate::dataflow_cache::DataflowCache;

    /// Verify the semaphore constant matches the design doc value (8 = 2x 4 ARM cores).
    #[test]
    fn semaphore_permits_is_8() {
        assert_eq!(SEMAPHORE_PERMITS, 8);
    }

    /// Verify that WALK_SEMAPHORE never exceeds SEMAPHORE_PERMITS available
    /// permits. Other concurrent tests may hold permits, so we assert `<=`
    /// rather than `==` (the global semaphore is shared across all tests).
    #[test]
    fn walk_semaphore_initial_permits() {
        assert!(
            WALK_SEMAPHORE.available_permits() <= SEMAPHORE_PERMITS,
            "available_permits {} should not exceed SEMAPHORE_PERMITS {}",
            WALK_SEMAPHORE.available_permits(),
            SEMAPHORE_PERMITS
        );
    }

    /// Acquire on a saturated semaphore must fail fast with HTTP 503
    /// (backpressure) instead of queueing forever. Reverting to an untimed
    /// `.acquire().await` makes this test hang (timeout → test failure).
    #[tokio::test]
    async fn acquire_walk_permit_returns_503_on_saturated_semaphore() {
        let sem = Semaphore::new(1);
        // Saturate the single permit.
        let _blocker = sem.acquire().await.unwrap();

        let result = acquire_walk_permit(&sem, Duration::from_millis(50)).await;
        assert!(
            result.is_err(),
            "saturated semaphore should return an error"
        );
        let (status, msg) = result.unwrap_err();
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "saturated pool should return 503"
        );
        assert!(
            msg.contains("saturated"),
            "error message should mention saturation: {msg}"
        );
    }

    /// Acquire on a semaphore with available permits succeeds normally.
    #[tokio::test]
    async fn acquire_walk_permit_succeeds_when_permits_available() {
        let sem = Semaphore::new(2);
        let permit = acquire_walk_permit(&sem, Duration::from_millis(100))
            .await
            .expect("permits available, should succeed");
        assert_eq!(sem.available_permits(), 1, "one permit should be held");
        drop(permit);
        assert_eq!(sem.available_permits(), 2, "permit released");
    }

    /// WalkMetrics tracks in-flight walk count and oldest start timestamp.
    /// Reverting WalkGuard (removing the increment/decrement) REDS this test.
    #[test]
    fn walk_metrics_tracks_in_flight() {
        let metrics = WalkMetrics::new();
        let (in_flight, oldest) = metrics.stats();
        assert_eq!(in_flight, 0);
        assert_eq!(oldest, 0);

        let slot1 = metrics.acquire_slot();
        let (in_flight, oldest) = metrics.stats();
        assert_eq!(in_flight, 1);
        assert!(oldest > 0, "oldest_start_ms should be set on first acquire");

        let slot2 = metrics.acquire_slot();
        let (in_flight, _) = metrics.stats();
        assert_eq!(in_flight, 2);

        drop(slot1);
        let (in_flight, _) = metrics.stats();
        assert_eq!(in_flight, 1);

        drop(slot2);
        let (in_flight, oldest) = metrics.stats();
        assert_eq!(in_flight, 0, "in_flight should return to 0");
        assert_eq!(
            oldest, 0,
            "oldest_start_ms should clear when no walks remain"
        );
    }

    /// Dedicated test-only statics for `concurrent_walks_through_real_path_drop_balances`.
    /// These are NOT shared with any other test (unlike `WALK_SEMAPHORE`/`WALK_METRICS`),
    /// so exact-count assertions on them are deterministic regardless of parallel
    /// test interleaving. `'static` references are required because `SemaphorePermit`
    /// and `WalkSlot` are moved into `spawn_blocking` closures.
    static TEST_SEM: Semaphore = Semaphore::const_new(4);
    static TEST_METRICS: WalkMetrics = WalkMetrics::new_const();

    /// Concurrency stress test through the REAL walk path:
    /// `acquire_walk_permit` + `WalkMetrics::acquire_slot` + `spawn_blocking`
    /// with the `WalkSlot` moved into the closure (so Drop runs on panic).
    ///
    /// Robustness approach — DEDICATED TEST-ONLY STATIC instances:
    /// `WALK_SEMAPHORE` and `WALK_METRICS` are process-global statics shared
    /// across parallel-running tests (this is why `walk_semaphore_initial_permits`
    /// had to weaken `==`→`<=`). To avoid flakiness, this test uses DEDICATED
    /// `static` items (`TEST_SEM`, `TEST_METRICS`) that NO other test touches —
    /// so exact-count assertions (in_flight → 0, Drop-balance) are deterministic
    /// regardless of parallel test interleaving. The REAL functions
    /// (`acquire_walk_permit`, `acquire_slot`, `WalkSlot::drop`) are exercised
    /// on `'static` references (required for `spawn_blocking`). No
    /// `serial_test` dep needed; the test is fully deterministic.
    ///
    /// Reverting any of the three mechanisms (untimed acquire, missing
    /// acquire_slot, or WalkSlot not moved into the closure) REDS this test:
    /// (a) untimed acquire → the 503 saturation sub-case hangs; (b) missing
    /// acquire_slot → in_flight stays 0, the return-to-zero assertion is
    /// vacuous; (c) WalkSlot not moved into the closure → the panic task
    /// drops the slot before the closure runs, in_flight decrements too early
    /// and the "return to 0 after all tasks" assertion can race.
    #[tokio::test]
    async fn concurrent_walks_through_real_path_drop_balances() {
        // ── Sub-case (a): N concurrent tasks through the real path, including
        // a panicking closure whose WalkSlot must still Drop (in_flight → 0).
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3u8 {
            let p = dir.path().join(format!("f{i}.ts"));
            std::fs::write(&p, b"const x = 1;").unwrap();
        }
        let root = dir.path().to_string_lossy().into_owned();

        let mut handles = Vec::new();
        for task_i in 0..8u8 {
            let r = root.clone();
            let h = tokio::spawn(async move {
                // Real path: bounded-timeout acquire (not raw .acquire().await).
                let permit = acquire_walk_permit(&TEST_SEM, Duration::from_secs(5))
                    .await
                    .expect("permits available for 8 tasks on a 4-permit sem");
                // Real path: acquire_slot, moved into spawn_blocking so Drop
                // runs when the closure finishes (even on panic).
                let slot = TEST_METRICS.acquire_slot();

                if task_i == 7 {
                    // Panicking closure: WalkSlot must still Drop during unwind.
                    let join_result = tokio::task::spawn_blocking(move || {
                        let _slot = slot;
                        let _permit = permit;
                        panic!("intentional panic in walk closure");
                    })
                    .await;
                    assert!(
                        join_result.is_err(),
                        "panic task should propagate as JoinError"
                    );
                    None
                } else {
                    let cache = DataflowCache::new();
                    let input = DataflowInput {
                        root: r,
                        language: "typescript".to_string(),
                        max_results: 100,
                        max_files: Some(10_000),
                        file_glob: None,
                        exclude_glob: None,
                    };
                    let resp = tokio::task::spawn_blocking(move || {
                        let _slot = slot;
                        let _permit = permit;
                        analyze_directory(input, &cache)
                    })
                    .await
                    .expect("task should not panic")
                    .expect("analysis should succeed");
                    Some(resp)
                }
            });
            handles.push(h);
        }

        let mut ok_completed = 0usize;
        let mut panic_completed = 0usize;
        for h in handles {
            match h.await.expect("outer task panicked") {
                Some(resp) => {
                    assert_eq!(resp.files_analyzed, 3);
                    ok_completed += 1;
                }
                None => panic_completed += 1,
            }
        }
        assert_eq!(ok_completed, 7, "7 non-panic tasks should complete");
        assert_eq!(panic_completed, 1, "1 panic task should report JoinError");

        // (b) After ALL tasks complete (including the panicking one), in_flight
        // must be back to 0 — guards a leaked WalkSlot. This is exact-count
        // safe because `TEST_METRICS` is a dedicated static no other test
        // touches (unlike the shared `WALK_METRICS`).
        let (in_flight, _) = TEST_METRICS.stats();
        assert_eq!(
            in_flight, 0,
            "in_flight must return to 0 after all tasks (including panic) complete; \
             a non-zero value means a WalkSlot leaked (Drop did not run)"
        );

        // ── Sub-case (c): under a saturated permit set, at least one acquire
        // returns 503 rather than hanging. Uses a local semaphore (no 'static
        // requirement — the permit is not moved into spawn_blocking here).
        let sem2 = Semaphore::new(1);
        let _blocker = sem2.acquire().await.unwrap(); // saturate the single permit
        let result = acquire_walk_permit(&sem2, Duration::from_millis(50)).await;
        assert!(
            result.is_err(),
            "saturated semaphore should return an error, not hang"
        );
        let (status, msg) = result.unwrap_err();
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "saturated pool should return 503"
        );
        assert!(
            msg.contains("saturated"),
            "error message should mention saturation: {msg}"
        );
    }
}
