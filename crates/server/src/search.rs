use axum::Json;
use axum::http::StatusCode;
use ox_core::grep;
use ox_core::{ExpandedSearchResponse, SearchInput};

pub async fn handle(
    Json(input): Json<SearchInput>,
) -> Result<Json<ExpandedSearchResponse>, (StatusCode, String)> {
    let result = tokio::task::spawn_blocking(move || grep::search(input))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(result))
}
