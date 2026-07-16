use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use ox_core::grep_filter::build_globset;
use ox_core::walk::{DEFAULT_FILE_COUNT_CAP, DEFAULT_MAX_FILE_BYTES, WalkBudget, filtered_walk};
use ox_dataflow::cfg_builder::build_cfg;
use ox_dataflow::il_builder::build_il_with_ext;
use ox_dataflow::taint::{TaintFinding, analyze_taint};
use ox_dataflow::taint_rules::{TaintRule, default_rules};
use ox_langs::get_language;

use crate::dataflow::clamp_max_files;
use crate::walk_guard::guarded_walk;

#[derive(Debug, Deserialize)]
pub struct TaintInput {
    pub root: String,
    pub language: String,
    #[serde(default)]
    pub rules: Option<Vec<TaintRule>>,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    /// Hard cap on files walked. Defaults to
    /// [`ox_core::walk::DEFAULT_FILE_COUNT_CAP`] (2000). An explicit JSON
    /// `null` deserializes to `None` (walks everything) — the server clamps
    /// both `null` and oversized values to its transport max.
    #[serde(default = "default_max_files")]
    pub max_files: Option<usize>,
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

fn default_max_files() -> Option<usize> {
    Some(DEFAULT_FILE_COUNT_CAP)
}

pub async fn handle(
    Json(mut input): Json<TaintInput>,
) -> Result<Json<TaintResponse>, (StatusCode, String)> {
    input.max_files = clamp_max_files(input.max_files);
    let result = guarded_walk(move || analyze(input)).await?;
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

    let budget = WalkBudget {
        max_files: input.max_files,
        // #52: files >20MB are skipped pre-read by `ignore`'s `max_filesize`.
        max_file_bytes: Some(DEFAULT_MAX_FILE_BYTES),
    };
    for (path, rel_path, _metadata) in filtered_walk(
        root,
        Some(lang_cfg.extensions),
        include.as_ref(),
        exclude.as_ref(),
        budget,
    ) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let source = match std::fs::read(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let il = match build_il_with_ext(&source, &input.language, ext) {
            Ok(il) => il,
            Err(_) => continue,
        };

        files_count += 1;
        for func in &il.functions {
            let cfg = build_cfg(func);
            findings.extend(analyze_taint(&cfg, &rules, &rel_path));
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn taint_input(root: &str) -> TaintInput {
        TaintInput {
            root: root.into(),
            language: "go".into(),
            rules: None,
            max_results: 100,
            max_files: Some(DEFAULT_FILE_COUNT_CAP),
            file_glob: None,
            exclude_glob: None,
        }
    }

    /// Golden / result-identity test: adopting `filtered_walk` + `guarded_walk`
    /// did NOT change what `/dataflow/taint` returns for a normal request.
    /// `files_analyzed` must match the number of qualifying .go files and the
    /// response shape must be consistent.
    #[test]
    fn test_taint_golden_result_identity() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("a.go"),
            "package main\nfunc a() {\n    x := source()\n    sink(x)\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b.go"),
            "package main\nfunc b() {\n    y := source()\n    sink(y)\n}\n",
        )
        .unwrap();

        let input = taint_input(&dir.path().to_string_lossy());
        let result = analyze(input).unwrap();
        assert_eq!(
            result.files_analyzed, 2,
            "golden: both .go files must be analyzed"
        );
        assert_eq!(
            result.total_findings,
            result.findings.len(),
            "golden: total_findings must equal findings.len() when not truncated"
        );
        assert!(
            !result.truncated,
            "golden: result must not be truncated for a small fixture"
        );
    }

    /// Byte-cap test (#52): a file >20MB is skipped pre-read by `ignore`'s
    /// `max_filesize`. Only the normal file is analyzed.
    /// Reverting `max_file_bytes` to `None` REDS this test (files_analyzed → 2).
    #[test]
    fn test_taint_skips_file_over_20mb() {
        let dir = TempDir::new().unwrap();
        // Normal file.
        std::fs::write(
            dir.path().join("small.go"),
            "package main\nfunc small() {\n    x := source()\n    sink(x)\n}\n",
        )
        .unwrap();
        // Big file (>20MB) padded with a long block comment.
        let padding = " ".repeat(21 * 1024 * 1024);
        let big_content = format!(
            "package main\nfunc big() {{\n    x := source()\n    sink(x)\n}}\n/* {padding} */\n"
        );
        std::fs::write(dir.path().join("big.go"), big_content).unwrap();

        let input = taint_input(&dir.path().to_string_lossy());
        let result = analyze(input).unwrap();
        assert_eq!(
            result.files_analyzed, 1,
            "big file (>20MB) must be skipped by the byte cap; expected files_analyzed=1"
        );
    }

    /// Serde default: an absent `max_files` field deserializes to
    /// `Some(DEFAULT_FILE_COUNT_CAP)` (2000), not `None`.
    #[test]
    fn test_taint_max_files_default_is_2000() {
        let json = r#"{"root":"/tmp","language":"go"}"#;
        let input: TaintInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.max_files, Some(DEFAULT_FILE_COUNT_CAP));
        assert_eq!(input.max_files, Some(2000));
    }

    /// Explicit `null` deserializes to `None` (the server clamps it to
    /// `MAX_MAX_FILES` via `clamp_max_files`).
    #[test]
    fn test_taint_max_files_null_is_none() {
        let json = r#"{"root":"/tmp","language":"go","max_files":null}"#;
        let input: TaintInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.max_files, None);
    }
}
