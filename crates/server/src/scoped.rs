use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use ox_core::scoped;
use ox_core::{ExpandedSearchResponse, ScopedSearchInput};

use crate::dataflow::clamp_max_files;
use crate::walk_guard::guarded_walk;

pub async fn handle(
    State(state): State<crate::AppState>,
    Json(mut input): Json<ScopedSearchInput>,
) -> Result<Json<ExpandedSearchResponse>, (StatusCode, String)> {
    input.max_files = clamp_max_files(input.max_files);
    let scope_cache = state.scope_cache.clone();
    let result = guarded_walk(move || scoped::scoped_search(input, &scope_cache)).await?;
    Ok(Json(result))
}
