use axum::Json;
use axum::http::StatusCode;
use ox_core::rewrite;
use ox_core::{RewriteInput, RewriteResponse};

use crate::dataflow::clamp_max_files;
use crate::walk_guard::guarded_walk;

pub async fn handle(
    Json(mut input): Json<RewriteInput>,
) -> Result<Json<RewriteResponse>, (StatusCode, String)> {
    input.max_files = clamp_max_files(input.max_files);
    let result = guarded_walk(move || rewrite::rewrite(input)).await?;
    Ok(Json(result))
}
