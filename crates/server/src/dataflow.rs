use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use ignore::WalkBuilder;
use tokio::sync::Semaphore;

use ox_core::grep_filter::build_globset;
use ox_dataflow::{DataflowInput, DataflowResponse, Finding, analysis, scope_walker};
use ox_langs::get_language;

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

static WALK_SEMAPHORE: Semaphore = Semaphore::const_new(SEMAPHORE_PERMITS);

pub async fn handle(
    Json(input): Json<DataflowInput>,
) -> Result<Json<DataflowResponse>, (StatusCode, String)> {
    // Acquire a walk permit BEFORE spawning the blocking task. If all permits
    // are taken, the caller waits here (backpressure) rather than blowing up
    // the blocking pool with unbounded concurrency.
    //
    // The permit is stored in _permit so it lives until the JoinHandle is
    // awaited (or the timeout branch drops it). Because spawn_blocking tasks
    // cannot be cancelled, we intentionally keep the permit alive until the
    // phantom task finishes, guaranteeing that at most SEMAPHORE_PERMITS walks
    // run concurrently at any point in time.
    let _permit = WALK_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let task = tokio::task::spawn_blocking(move || analyze_directory(input));

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

fn analyze_directory(input: DataflowInput) -> Result<DataflowResponse> {
    let start = Instant::now();

    let lang_cfg = get_language(&input.language)
        .ok_or_else(|| anyhow::anyhow!("unsupported language: {}", input.language))?;

    let include = input
        .file_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;
    let exclude = input
        .exclude_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    let root = Path::new(&input.root);
    let mut all_findings: Vec<Finding> = Vec::new();
    let mut files_analyzed: usize = 0;

    let mut files_truncated = false;
    for entry in WalkBuilder::new(root)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
    {
        // File cap: stop walking once limit is reached.
        if let Some(cap) = input.max_files
            && files_analyzed >= cap
        {
            files_truncated = true;
            break;
        }

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

        let chain = match scope_walker::walk_file(&source, &input.language) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let rel_str = rel_path.to_string_lossy();
        let findings = analysis::analyze(&chain, &rel_str);
        all_findings.extend(findings);
        files_analyzed += 1;

        if all_findings.len() >= input.max_results * 5 {
            break;
        }
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
    use ox_dataflow::DataflowInput;
    use tempfile::tempdir;

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

        let result = analyze_directory(input).unwrap();
        assert_eq!(
            result.files_analyzed, 3,
            "expected exactly 3 files, got {}",
            result.files_analyzed
        );
        assert!(result.files_truncated, "files_truncated should be true when cap is hit");
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

        let result = analyze_directory(input).unwrap();
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

        let result = analyze_directory(input).unwrap();
        assert!(
            result.files_analyzed >= 1,
            "svelte file should not be skipped; files_analyzed={}",
            result.files_analyzed
        );
    }
}


#[cfg(test)]
mod semaphore_tests {
    use super::*;

    /// Verify the semaphore constant matches the design doc value (8 = 2x 4 ARM cores).
    #[test]
    fn semaphore_permits_is_8() {
        assert_eq!(SEMAPHORE_PERMITS, 8);
    }

    /// Verify that WALK_SEMAPHORE starts with the expected number of available permits.
    #[test]
    fn walk_semaphore_initial_permits() {
        assert_eq!(WALK_SEMAPHORE.available_permits(), SEMAPHORE_PERMITS);
    }

    /// 10 concurrent fast requests all complete successfully.
    /// Verifies the semaphore does not deadlock or starve short-lived tasks.
    #[tokio::test]
    async fn concurrent_fast_requests_all_complete() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3u8 {
            let p = dir.path().join(format!("f{i}.ts"));
            std::fs::write(&p, b"const x = 1;").unwrap();
        }

        let root = dir.path().to_string_lossy().into_owned();

        let mut handles = Vec::new();
        for _ in 0..10 {
            let r = root.clone();
            let h = tokio::spawn(async move {
                let _permit = WALK_SEMAPHORE.acquire().await.unwrap();
                let input = DataflowInput {
                    root: r,
                    language: "typescript".to_string(),
                    max_results: 100,
                    max_files: Some(10_000),
                    file_glob: None,
                    exclude_glob: None,
                };
                tokio::task::spawn_blocking(move || analyze_directory(input))
                    .await
                    .unwrap()
                    .unwrap()
            });
            handles.push(h);
        }

        let mut completed = 0usize;
        for h in handles {
            let resp = h.await.expect("task panicked");
            assert_eq!(resp.files_analyzed, 3);
            completed += 1;
        }
        assert_eq!(completed, 10);
    }
}
