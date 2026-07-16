use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use ox_core::rewrite;
use ox_core::{RewriteInput, RewriteResponse};

use crate::dataflow::clamp_max_files;
use crate::walk_guard::{REWRITE_TIMEOUT_SECS, guarded_walk_with_deadline};

/// `POST /rewrite` — structural search + transform (mutates files on disk when
/// `apply=true`).
///
/// Runs under a generous `REWRITE_TIMEOUT_SECS` deadline (not the 25s read-path
/// deadline) so a legitimate bulk rewrite completes and returns 200 rather than
/// falsely 504'ing mid-apply. NOTE: the blocking-pool task is uncancellable, so
/// a `504` from this endpoint means the rewrite MAY have partially or fully
/// applied — `/rewrite` is not yet idempotent, so a caller that receives a 504
/// must RE-READ the affected files before retrying, not assume nothing changed.
pub async fn handle(
    Json(mut input): Json<RewriteInput>,
) -> Result<Json<RewriteResponse>, (StatusCode, String)> {
    input.max_files = clamp_max_files(input.max_files);
    let result = guarded_walk_with_deadline(Duration::from_secs(REWRITE_TIMEOUT_SECS), move || {
        rewrite::rewrite(input)
    })
    .await?;
    Ok(Json(result))
}
