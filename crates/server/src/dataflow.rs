use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use ignore::WalkBuilder;

use ox_core::grep_filter::build_globset;
use ox_dataflow::{DataflowInput, DataflowResponse, Finding, analysis, scope_walker};
use ox_langs::get_language;

pub async fn handle(
    Json(input): Json<DataflowInput>,
) -> Result<Json<DataflowResponse>, (StatusCode, String)> {
    let result = tokio::task::spawn_blocking(move || analyze_directory(input))
        .await
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
        if let Some(ref inc) = include {
            if !inc.is_match(rel_path) {
                continue;
            }
        }
        if let Some(ref exc) = exclude {
            if exc.is_match(rel_path) {
                continue;
            }
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
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
