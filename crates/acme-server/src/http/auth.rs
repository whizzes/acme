//! Per-dialect bearer auth (spec §10.1: `Authorization: Bearer sk_test_…`).
//! Resolves the credential's `merchant_id` and stashes it as a request
//! extension for downstream handlers and `http::idempotency` to read —
//! nothing under `providers/` queries `api_credentials` directly (spec §5).

use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
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
                tracing::debug!(provider_slug = auth.provider_slug, %merchant_id, "bearer auth: authenticated");
                req.extensions_mut()
                    .insert(AuthenticatedMerchant(merchant_id));
                next.run(req).await
            }
            Err(_) => {
                tracing::error!(
                    provider_slug = auth.provider_slug,
                    merchant_id,
                    "bearer auth: stored merchant_id does not parse"
                );
                AcmeError::Internal(anyhow::anyhow!(
                    "stored merchant_id `{merchant_id}` does not parse"
                ))
                .into_response()
            }
        },
        Ok(None) => {
            tracing::warn!(
                provider_slug = auth.provider_slug,
                "bearer auth: rejected, no active credential matches token"
            );
            AcmeError::Unauthorized("invalid API credential").into_response()
        }
        Err(error) => {
            tracing::error!(
                ?error,
                provider_slug = auth.provider_slug,
                "bearer auth: credential lookup failed"
            );
            AcmeError::Internal(error.into()).into_response()
        }
    }
}

/// Static header key-pair auth (spec §10.2: `Tbk-Api-Key-Id`/
/// `Tbk-Api-Key-Secret`, both static, no expiry) — `api_credentials.
/// public_key` holds the id, `secret_key` holds the secret, same table
/// `bearer_auth` reads, just matched on two headers instead of one.
pub async fn header_key_pair_auth(
    State(auth): State<HeaderKeyPairState>,
    mut req: Request,
    next: Next,
) -> Response {
    let key_id = req
        .headers()
        .get(auth.id_header)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let key_secret = req
        .headers()
        .get(auth.secret_header)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let (Some(key_id), Some(key_secret)) = (key_id, key_secret) else {
        return AcmeError::Unauthorized("missing API key headers").into_response();
    };

    let row: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
        "SELECT merchant_id FROM api_credentials
         WHERE public_key = ?1 AND secret_key = ?2 AND provider_slug = ?3 AND active = 1",
    )
    .bind(&key_id)
    .bind(&key_secret)
    .bind(auth.provider_slug)
    .fetch_optional(&auth.pool)
    .await;

    match row {
        Ok(Some((merchant_id,))) => match merchant_id.parse::<MerchantId>() {
            Ok(merchant_id) => {
                tracing::debug!(provider_slug = auth.provider_slug, %merchant_id, "header key-pair auth: authenticated");
                req.extensions_mut()
                    .insert(AuthenticatedMerchant(merchant_id));
                next.run(req).await
            }
            Err(_) => {
                tracing::error!(
                    provider_slug = auth.provider_slug,
                    merchant_id,
                    "header key-pair auth: stored merchant_id does not parse"
                );
                AcmeError::Internal(anyhow::anyhow!(
                    "stored merchant_id `{merchant_id}` does not parse"
                ))
                .into_response()
            }
        },
        Ok(None) => {
            tracing::warn!(
                provider_slug = auth.provider_slug,
                "header key-pair auth: rejected, no active credential matches key id/secret"
            );
            AcmeError::Unauthorized("invalid API key pair").into_response()
        }
        Err(error) => {
            tracing::error!(
                ?error,
                provider_slug = auth.provider_slug,
                "header key-pair auth: credential lookup failed"
            );
            AcmeError::Internal(error.into()).into_response()
        }
    }
}

#[derive(Clone)]
pub struct HeaderKeyPairState {
    pub pool: SqlitePool,
    pub provider_slug: &'static str,
    pub id_header: &'static str,
    pub secret_header: &'static str,
}

/// Validates a bearer token issued by an OAuth2 grant (spec §11.2's
/// password grant) against `db::repo::oauth_tokens`, not
/// `api_credentials` — the token is minted per session by the dialect's
/// own `POST …/token` handler, not a static secret a merchant holds.
/// Same request/response shape as `bearer_auth`, deliberately: a dialect
/// switching from a static credential to an OAuth2 flow changes only
/// *how* a token comes to exist, not how a request carrying one is
/// checked.
pub async fn oauth2_bearer_auth(
    State(auth): State<OAuth2State>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer_token(&req) else {
        return AcmeError::Unauthorized("missing bearer token").into_response();
    };

    match crate::db::repo::oauth_tokens::validate(&auth.pool, token, auth.provider_slug, Utc::now())
        .await
    {
        Ok(Some(merchant_id)) => {
            tracing::debug!(provider_slug = auth.provider_slug, %merchant_id, "oauth2 bearer auth: authenticated");
            req.extensions_mut()
                .insert(AuthenticatedMerchant(merchant_id));
            next.run(req).await
        }
        Ok(None) => {
            tracing::warn!(
                provider_slug = auth.provider_slug,
                "oauth2 bearer auth: rejected, token invalid or expired"
            );
            AcmeError::Unauthorized("invalid or expired access token").into_response()
        }
        Err(error) => {
            tracing::error!(
                ?error,
                provider_slug = auth.provider_slug,
                "oauth2 bearer auth: token lookup failed"
            );
            AcmeError::Internal(error).into_response()
        }
    }
}

#[derive(Clone)]
pub struct OAuth2State {
    pub pool: SqlitePool,
    pub provider_slug: &'static str,
}

/// Checks an OAuth2 password grant's four credentials against
/// `api_credentials` (spec §11.2's Iberex) — `client_id`/`client_secret`
/// against `public_key`/`secret_key`, `username`/`password` against JSON
/// stashed in `extra` (there's no dedicated username/password column,
/// spec §7.2's `extra` field being exactly the escape hatch for a
/// dialect-specific extra credential this is). Lives here rather than in
/// `providers::shipping::iberex::routes` because this file — like
/// `capture/` — is a pre-existing, deliberate exception to the "`db::repo`
/// is the only code allowed to query" rule (spec §5,
/// `tests/architecture.rs`'s own header comment): auth is the one place a
/// provider-facing token exchange still needs to check a credential
/// directly, the same way `bearer_auth` and `header_key_pair_auth` above
/// already do.
pub async fn validate_oauth2_password_grant(
    pool: &SqlitePool,
    provider_slug: &str,
    client_id: &str,
    client_secret: &str,
    username: &str,
    password: &str,
) -> anyhow::Result<Option<MerchantId>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT merchant_id, extra FROM api_credentials
         WHERE public_key = ?1 AND secret_key = ?2 AND provider_slug = ?3 AND active = 1",
    )
    .bind(client_id)
    .bind(client_secret)
    .bind(provider_slug)
    .fetch_optional(pool)
    .await?;

    let Some((merchant_id, extra)) = row else {
        tracing::warn!(
            provider_slug,
            client_id,
            "oauth2 password grant: rejected, no active credential matches client_id/client_secret"
        );
        return Ok(None);
    };

    let credentials_match = extra
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .is_some_and(|v| {
            v.get("username").and_then(|u| u.as_str()) == Some(username)
                && v.get("password").and_then(|p| p.as_str()) == Some(password)
        });
    if !credentials_match {
        tracing::warn!(
            provider_slug,
            client_id,
            "oauth2 password grant: rejected, username/password mismatch"
        );
        return Ok(None);
    }

    tracing::debug!(provider_slug, client_id, %merchant_id, "oauth2 password grant: issuing token");
    Ok(merchant_id.parse().ok())
}
