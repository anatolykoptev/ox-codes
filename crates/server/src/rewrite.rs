use axum::Json;
use axum::http::StatusCode;
use ox_core::rewrite;
use ox_core::{RewriteInput, RewriteResponse};

pub async fn handle(
    Json(input): Json<RewriteInput>,
) -> Result<Json<RewriteResponse>, (StatusCode, String)> {
    let result = tokio::task::spawn_blocking(move || rewrite::rewrite(input))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(result))
}
