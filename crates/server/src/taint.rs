use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

use ox_core::grep_filter::build_globset;
use ox_dataflow::cfg_builder::build_cfg;
use ox_dataflow::il_builder::build_il;
use ox_dataflow::taint::{TaintFinding, analyze_taint};
use ox_dataflow::taint_rules::{TaintRule, default_rules};
use ox_langs::get_language;

#[derive(Debug, Deserialize)]
pub struct TaintInput {
    pub root: String,
    pub language: String,
    #[serde(default)]
    pub rules: Option<Vec<TaintRule>>,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default)]
    pub file_glob: Option<String>,
    #[serde(default)]
    pub exclude_glob: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TaintResponse {
    pub findings: Vec<TaintFinding>,
    pub total_findings: usize,
    pub files_analyzed: usize,
    pub truncated: bool,
    pub duration_ms: u64,
}

fn default_max_results() -> usize {
    100
}

pub async fn handle(
    Json(input): Json<TaintInput>,
) -> Result<Json<TaintResponse>, (StatusCode, String)> {
    let result = tokio::task::spawn_blocking(move || analyze(input))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(result))
}

fn analyze(input: TaintInput) -> Result<TaintResponse> {
    let start = Instant::now();
    let rules = input
        .rules
        .unwrap_or_else(|| default_rules(&input.language));

    let lang_cfg = get_language(&input.language)
        .ok_or_else(|| anyhow::anyhow!("unsupported language: {}", input.language))?;

    let include = input.file_glob.as_deref().map(build_globset).transpose()?;
    let exclude = input
        .exclude_glob
        .as_deref()
        .map(build_globset)
        .transpose()?;

    let root = Path::new(&input.root);
    let mut findings = Vec::new();
    let mut files_count = 0;

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
        let il = match build_il(&source, &input.language) {
            Ok(il) => il,
            Err(_) => continue,
        };

        files_count += 1;
        let file_str = rel_path.to_string_lossy().to_string();
        for func in &il.functions {
            let cfg = build_cfg(func);
            findings.extend(analyze_taint(&cfg, &rules, &file_str));
        }

        if findings.len() >= input.max_results * 5 {
            break;
        }
    }

    let total = findings.len();
    let truncated = total > input.max_results;
    if truncated {
        findings.truncate(input.max_results);
    }

    Ok(TaintResponse {
        findings,
        total_findings: total,
        files_analyzed: files_count,
        truncated,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
