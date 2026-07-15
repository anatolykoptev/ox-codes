use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use ox_core::scoped;
use ox_core::{ExpandedSearchResponse, ScopedSearchInput};

pub async fn handle(
    State(state): State<crate::AppState>,
    Json(input): Json<ScopedSearchInput>,
) -> Result<Json<ExpandedSearchResponse>, (StatusCode, String)> {
    let scope_cache = state.scope_cache.clone();
    let result = tokio::task::spawn_blocking(move || scoped::scoped_search(input, &scope_cache))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(result))
}
