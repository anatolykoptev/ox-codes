use axum::Json;
use axum::http::StatusCode;
use ox_core::structural;
use ox_core::{ExpandedSearchResponse, StructuralSearchInput};

use crate::dataflow::clamp_max_files;
use crate::walk_guard::guarded_walk;

pub async fn handle(
    Json(mut input): Json<StructuralSearchInput>,
) -> Result<Json<ExpandedSearchResponse>, (StatusCode, String)> {
    input.max_files = clamp_max_files(input.max_files);
    let result = guarded_walk(move || structural::structural_search(input)).await?;
    Ok(Json(result))
}
