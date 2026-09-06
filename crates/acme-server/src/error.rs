//! Fallback error type for the dashboard's own routes (anyhow -> 500), plus
//! `AcmeError` (spec §8.5) — the error enum provider adapters use from M2.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

use crate::domain::ids::FaultId;

/// Acme Pay/Acme Ship's shared error envelope (spec §8.5): the OpenAPI-facing
/// twin of the ad hoc `serde_json::json!` body `AcmeError` actually renders,
/// kept as a named schema so client generators don't get an anonymous type.
#[derive(Debug, Serialize, ToSchema)]
pub struct AcmeErrorBody {
    pub error: AcmeErrorDetail,
}

/// One error's details: a stable machine-readable `code`, a human
/// `message`, and — for validation errors — which request `param` it
/// concerns.
#[derive(Debug, Serialize, ToSchema)]
pub struct AcmeErrorDetail {
    /// Always equal to `code` — kept for Stripe-shaped client compatibility.
    #[serde(rename = "type")]
    pub kind: String,
    /// Stable machine-readable error code, e.g. `validation_error`.
    pub code: String,
    /// Human-readable explanation, safe to show a developer.
    pub message: String,
    /// The request field this error concerns, when applicable.
    pub param: Option<String>,
    /// Deep link into the dashboard's request log for this call.
    pub request_id: Option<String>,
}

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

#[derive(Debug, Clone)]
pub struct FieldError {
    pub param: String,
    pub message: String,
}

/// One internal error enum, rendered per dialect once a dialect exists.
/// Before M2, only Acme Pay's own envelope is wired — see
/// `into_response_with_request_id`.
#[derive(Debug)]
pub enum AcmeError {
    Unauthorized(&'static str),
    Forbidden,
    NotFound(&'static str),
    Validation(Vec<FieldError>),
    Conflict(String),
    UnprocessableState {
        from: String,
        to: String,
    },
    RateLimited {
        retry_after_s: u64,
    },
    ProviderDown,
    Injected(FaultId),
    Internal(anyhow::Error),
    /// A payment was declined synchronously (spec §14's own path example
    /// documents `402` for this, added in M2 — see this file's own header
    /// note that `AcmeError` is "the error enum provider adapters use from
    /// M2", i.e. grown as adapters need it). The payment row still exists
    /// with `status: rejected`; only the *create* response is an error.
    Declined {
        code: String,
        message: String,
    },
}

impl AcmeError {
    pub fn status(&self) -> StatusCode {
        match self {
            AcmeError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AcmeError::Forbidden => StatusCode::FORBIDDEN,
            AcmeError::NotFound(_) => StatusCode::NOT_FOUND,
            AcmeError::Validation(_) => StatusCode::BAD_REQUEST,
            AcmeError::Conflict(_) => StatusCode::CONFLICT,
            AcmeError::UnprocessableState { .. } => StatusCode::CONFLICT,
            AcmeError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            AcmeError::ProviderDown => StatusCode::BAD_GATEWAY,
            AcmeError::Injected(_) => StatusCode::SERVICE_UNAVAILABLE,
            AcmeError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AcmeError::Declined { .. } => StatusCode::PAYMENT_REQUIRED,
        }
    }

    fn code(&self) -> String {
        match self {
            AcmeError::Unauthorized(_) => "unauthorized".to_string(),
            AcmeError::Forbidden => "forbidden".to_string(),
            AcmeError::NotFound(_) => "not_found".to_string(),
            AcmeError::Validation(_) => "validation_error".to_string(),
            AcmeError::Conflict(_) => "conflict".to_string(),
            AcmeError::UnprocessableState { .. } => "unprocessable_state".to_string(),
            AcmeError::RateLimited { .. } => "rate_limited".to_string(),
            AcmeError::ProviderDown => "provider_down".to_string(),
            AcmeError::Injected(_) => "fault_injected".to_string(),
            AcmeError::Internal(_) => "internal_error".to_string(),
            AcmeError::Declined { code, .. } => code.clone(),
        }
    }

    fn message(&self) -> String {
        match self {
            AcmeError::Unauthorized(msg) => msg.to_string(),
            AcmeError::Forbidden => "forbidden".to_string(),
            AcmeError::NotFound(what) => format!("{what} not found"),
            AcmeError::Validation(errors) => errors
                .first()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "validation failed".to_string()),
            AcmeError::Conflict(msg) => msg.clone(),
            AcmeError::UnprocessableState { from, to } => {
                format!("cannot transition from {from} to {to}")
            }
            AcmeError::RateLimited { retry_after_s } => {
                format!("rate limited, retry after {retry_after_s}s")
            }
            AcmeError::ProviderDown => "provider unavailable".to_string(),
            AcmeError::Injected(fault_id) => format!("fault {fault_id} injected"),
            AcmeError::Internal(_) => "internal error".to_string(),
            AcmeError::Declined { message, .. } => message.clone(),
        }
    }

    fn param(&self) -> Option<String> {
        match self {
            AcmeError::Validation(errors) => errors.first().map(|e| e.param.clone()),
            _ => None,
        }
    }

    /// Renders using Acme Pay's own envelope (spec §8.5's house dialect).
    /// `request_id` is `api_requests.id` in the full system; pass `None`
    /// where that context isn't available yet.
    pub fn into_response_with_request_id(self, request_id: Option<&str>) -> Response {
        if let AcmeError::Internal(ref err) = self {
            tracing::error!(error = ?err, "internal error");
        }

        let status = self.status();
        let body = serde_json::json!({
            "error": {
                "type": self.code(),
                "code": self.code(),
                "message": self.message(),
                "param": self.param(),
                "request_id": request_id,
            }
        });

        (status, axum::Json(body)).into_response()
    }
}

impl IntoResponse for AcmeError {
    fn into_response(self) -> Response {
        self.into_response_with_request_id(None)
    }
}

/// Every `db::repo` function returns `anyhow::Result`; provider handlers
/// propagate a failure with `?` straight into a `500`.
impl From<anyhow::Error> for AcmeError {
    fn from(err: anyhow::Error) -> Self {
        AcmeError::Internal(err)
    }
}

impl From<sqlx::Error> for AcmeError {
    fn from(err: sqlx::Error) -> Self {
        AcmeError::Internal(err.into())
    }
}

impl From<serde_json::Error> for AcmeError {
    fn from(err: serde_json::Error) -> Self {
        AcmeError::Internal(err.into())
    }
}

impl From<crate::domain::error::DomainError> for AcmeError {
    fn from(err: crate::domain::error::DomainError) -> Self {
        AcmeError::Internal(err.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_match_spec() {
        assert_eq!(
            AcmeError::NotFound("payment").status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AcmeError::Conflict("x".into()).status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AcmeError::RateLimited { retry_after_s: 1 }.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
}
