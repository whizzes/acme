//! Fallback error type for the dashboard's own routes (anyhow -> 500).
//! Distinct from the per-dialect `AcmeError` enum providers use from M2 (spec §8.5).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[allow(dead_code)]
pub struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::error!(error = ?self.0, "unhandled error");
        (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
