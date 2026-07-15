mod dataflow;
mod rewrite;
mod scoped;
mod search;
mod structural;
mod taint;

use axum::{
    Router,
    routing::{get, post},
};

#[derive(Clone)]
pub struct AppState {
    pub scope_cache: ox_core::ScopeCache,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
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
