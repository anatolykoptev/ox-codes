use axum::Json;
use axum::http::StatusCode;
use ox_core::{StructuralSearchInput, SearchResponse};
use ox_core::structural;

pub async fn handle(
    Json(input): Json<StructuralSearchInput>,
) -> Result<Json<SearchResponse>, (StatusCode, String)> {
    let result = tokio::task::spawn_blocking(move || structural::structural_search(input))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(result))
}
