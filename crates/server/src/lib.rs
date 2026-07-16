mod dataflow;
pub mod dataflow_cache;
mod rewrite;
mod scoped;
mod search;
mod structural;
mod taint;
mod walk_guard;

pub use dataflow_cache::DataflowCache;

use axum::{
    Router,
    extract::State,
    response::Json,
    routing::{get, post},
};
use serde::Serialize;

#[derive(Clone)]
pub struct AppState {
    pub scope_cache: ox_core::ScopeCache,
    pub dataflow_cache: DataflowCache,
}

/// Per-cache effectiveness counters.
#[derive(Debug, Serialize)]
struct CacheStatsEntry {
    hits: u64,
    misses: u64,
    entry_count: u64,
}

/// Walk-pool stats returned by `GET /cache/stats` → `walks`.
///
/// `in_flight` is the number of currently-active directory walks (permits
/// held). `oldest_start_ms` is the UNIX-epoch millisecond timestamp of the
/// oldest in-flight walk, or 0 if none. A walk whose age exceeds
/// `DATAFLOW_TIMEOUT_SECS` is "stuck" (its permit will never be returned
/// because `spawn_blocking` cannot be cancelled).
#[derive(Debug, Serialize)]
struct WalkStatsEntry {
    in_flight: u64,
    oldest_start_ms: u64,
}

/// Combined cache stats returned by `GET /cache/stats`.
#[derive(Debug, Serialize)]
struct CacheStatsResponse {
    scope: CacheStatsEntry,
    dataflow: CacheStatsEntry,
    walks: WalkStatsEntry,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/cache/stats", get(cache_stats))
        .route("/search", post(search::handle))
        .route("/search/scoped", post(scoped::handle))
        .route("/search/structural", post(structural::handle))
        .route("/rewrite", post(rewrite::handle))
        .route("/dataflow/analyze", post(dataflow::handle))
        .route("/dataflow/taint", post(taint::handle))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn cache_stats(State(state): State<AppState>) -> Json<CacheStatsResponse> {
    let (scope_hits, scope_misses) = state.scope_cache.stats();
    let (dataflow_hits, dataflow_misses, _dataflow_analyses) = state.dataflow_cache.stats();
    let (walk_in_flight, walk_oldest_start) = walk_guard::walk_metrics();

    Json(CacheStatsResponse {
        scope: CacheStatsEntry {
            hits: scope_hits,
            misses: scope_misses,
            entry_count: state.scope_cache.entry_count(),
        },
        dataflow: CacheStatsEntry {
            hits: dataflow_hits,
            misses: dataflow_misses,
            entry_count: state.dataflow_cache.entry_count(),
        },
        walks: WalkStatsEntry {
            in_flight: walk_in_flight,
            oldest_start_ms: walk_oldest_start,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json as AxumJson;
    use axum::extract::State;
    use ox_dataflow::{DataflowInput, DataflowResponse};

    #[tokio::test]
    async fn cache_stats_endpoint_reports_counters() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        std::fs::write(dir.path().join("file.ts"), b"const x = 1;").unwrap();

        let state = AppState {
            scope_cache: ox_core::ScopeCache::new(),
            dataflow_cache: DataflowCache::new(),
        };

        let input = DataflowInput {
            root: root.clone(),
            language: "typescript".into(),
            max_results: 100,
            max_files: Some(3),
            file_glob: None,
            exclude_glob: None,
        };

        let AxumJson(r1): AxumJson<DataflowResponse> =
            dataflow::handle(State(state.clone()), AxumJson(input.clone()))
                .await
                .unwrap();
        let AxumJson(r2): AxumJson<DataflowResponse> =
            dataflow::handle(State(state.clone()), AxumJson(input))
                .await
                .unwrap();

        assert!(
            r2.duration_ms < 100,
            "hit path should report a small duration_ms, got {}",
            r2.duration_ms
        );
        assert!(r1.duration_ms > 0 || r2.duration_ms > 0);

        let Json(stats) = cache_stats(State(state)).await;

        assert!(
            stats.dataflow.hits >= 1,
            "dataflow hits: {}",
            stats.dataflow.hits
        );
        assert!(
            stats.dataflow.misses >= 1,
            "dataflow misses: {}",
            stats.dataflow.misses
        );
        assert!(
            stats.dataflow.entry_count >= 1,
            "dataflow entry_count: {}",
            stats.dataflow.entry_count
        );
        assert_eq!(stats.scope.hits, 0);
        assert_eq!(stats.scope.misses, 0);
        assert_eq!(stats.scope.entry_count, 0);
    }
}
