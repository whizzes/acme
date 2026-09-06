//! Per-dialect bearer auth (spec §10.1: `Authorization: Bearer sk_test_…`).
//! Resolves the credential's `merchant_id` and stashes it as a request
//! extension for downstream handlers and `http::idempotency` to read —
//! nothing under `providers/` queries `api_credentials` directly (spec §5).

use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::SqlitePool;

use crate::domain::ids::MerchantId;
use crate::error::AcmeError;

#[derive(Clone)]
pub struct AuthState {
    pub pool: SqlitePool,
    pub provider_slug: &'static str,
}

/// The merchant a request authenticated as, inserted into the request's
/// extensions by `bearer_auth` and read back by handlers via
/// `Extension<AuthenticatedMerchant>`.
#[derive(Clone, Copy)]
pub struct AuthenticatedMerchant(pub MerchantId);

fn bearer_token(req: &Request) -> Option<&str> {
    req.headers()
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

pub async fn bearer_auth(State(auth): State<AuthState>, mut req: Request, next: Next) -> Response {
    let Some(token) = bearer_token(&req) else {
        return AcmeError::Unauthorized("missing bearer token").into_response();
    };

    let row: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
        "SELECT merchant_id FROM api_credentials WHERE secret_key = ?1 AND provider_slug = ?2 AND active = 1",
    )
    .bind(token)
    .bind(auth.provider_slug)
    .fetch_optional(&auth.pool)
    .await;

    match row {
        Ok(Some((merchant_id,))) => match merchant_id.parse::<MerchantId>() {
            Ok(merchant_id) => {
                req.extensions_mut()
                    .insert(AuthenticatedMerchant(merchant_id));
                next.run(req).await
            }
            Err(_) => AcmeError::Internal(anyhow::anyhow!(
                "stored merchant_id `{merchant_id}` does not parse"
            ))
            .into_response(),
        },
        Ok(None) => AcmeError::Unauthorized("invalid API credential").into_response(),
        Err(error) => AcmeError::Internal(error.into()).into_response(),
    }
}
