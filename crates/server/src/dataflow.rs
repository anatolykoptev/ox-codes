use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use ignore::WalkBuilder;

use ox_core::grep_filter::build_globset;
use ox_dataflow::{DataflowInput, DataflowResponse, Finding, analysis, scope_walker};
use ox_langs::get_language;

/// Hard deadline for a single dataflow request. Note: `spawn_blocking` cannot
/// be cancelled — the task continues running in the blocking thread pool after
/// the timeout fires, but the HTTP response is returned immediately.
const DATAFLOW_TIMEOUT_SECS: u64 = 25;

pub async fn handle(
    Json(input): Json<DataflowInput>,
) -> Result<Json<DataflowResponse>, (StatusCode, String)> {
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
