use super::Hook;
use axum::extract::State;
use axum::{debug_handler, extract::Path};
use http::StatusCode;
use lob::Encode;
use tracing::instrument;

#[instrument(skip_all, fields(symbol))]
#[debug_handler]
pub(super) async fn serve_book(
    Path(symbol): Path<String>,
    hook: State<Hook>,
) -> (StatusCode, Vec<u8>) {
    let lob = hook.get_or_default(&symbol).await;
    if let Some(lob) = lob {
        (StatusCode::OK, lob.encode())
    } else {
        (StatusCode::NOT_FOUND, lob.encode())
    }
}
