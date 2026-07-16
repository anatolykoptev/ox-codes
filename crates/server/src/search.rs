use axum::Json;
use axum::http::StatusCode;
use ox_core::grep;
use ox_core::{ExpandedSearchResponse, SearchInput};

use crate::dataflow::clamp_max_files;
use crate::walk_guard::guarded_walk;

pub async fn handle(
    Json(mut input): Json<SearchInput>,
) -> Result<Json<ExpandedSearchResponse>, (StatusCode, String)> {
    input.max_files = clamp_max_files(input.max_files);
    let result = guarded_walk(move || grep::search(input)).await?;
    Ok(Json(result))
}
