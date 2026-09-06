//! Idempotency middleware (spec §8.4), keyed on `Acme-Idempotency-Key`
//! (the header name both Acme Pay and Acme Ship use), backed by the
//! `idempotency_keys` table migrated in M1 and unused until now. Mounted
//! inside `http::auth::bearer_auth` on each provider router, so it always
//! runs with a resolved `AuthenticatedMerchant` already in the request's
//! extensions.

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::Duration;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::error::AcmeError;
use crate::http::auth::AuthenticatedMerchant;
use crate::sim::clock::SimClock;

const BODY_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct IdempotencyState {
    pub pool: SqlitePool,
    pub clock: SimClock,
}

/// Sorts every object's keys, recursively, before serializing — the
/// "canonicalized body" spec §8.4 step 1 hashes. Rebuilding into a fresh
/// `Map` from sorted `(key, value)` pairs guarantees sorted-key output
/// whether or not `serde_json`'s `preserve_order` feature is active
/// elsewhere in the build (utoipa turns it on for its own schema output).
fn canonicalize(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> =
                map.into_iter().map(|(k, v)| (k, canonicalize(v))).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(entries.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonicalize).collect())
        }
        other => other,
    }
}

fn canonical_hash(bytes: &[u8]) -> String {
    let canonical = match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(value) => serde_json::to_vec(&canonicalize(value)).unwrap_or_else(|_| bytes.to_vec()),
        Err(_) => bytes.to_vec(),
    };
    let mut hasher = Sha256::new();
    hasher.update(&canonical);
    hex::encode(hasher.finalize())
}

struct StoredResponse {
    request_hash: String,
    state: String,
    response_status: Option<i64>,
    response_body: Option<String>,
}

pub async fn layer(State(state): State<IdempotencyState>, req: Request, next: Next) -> Response {
    if req.method() == Method::GET {
        return next.run(req).await;
    }

    let Some(key) = req
        .headers()
        .get("acme-idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
    else {
        return next.run(req).await;
    };

    let Some(&AuthenticatedMerchant(merchant_id)) = req.extensions().get::<AuthenticatedMerchant>()
    else {
        // Auth runs outside this layer and would already have rejected an
        // unauthenticated request — nothing to key idempotency on here.
        return next.run(req).await;
    };

    let endpoint = format!("{} {}", req.method(), req.uri().path());
    let (parts, body) = req.into_parts();
    let request_bytes = to_bytes(body, BODY_LIMIT).await.unwrap_or_default();
    let hash = canonical_hash(&request_bytes);
    let req = Request::from_parts(parts, Body::from(request_bytes));

    let now = state.clock.now();
    let expires_at = now + Duration::hours(24);

    let insert = sqlx::query(
        "INSERT INTO idempotency_keys (merchant_id, endpoint, key, request_hash, state, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, 'in_flight', ?5, ?6)
         ON CONFLICT DO NOTHING",
    )
    .bind(merchant_id.to_string())
    .bind(&endpoint)
    .bind(&key)
    .bind(&hash)
    .bind(now.to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .execute(&state.pool)
    .await;

    let won = matches!(&insert, Ok(result) if result.rows_affected() == 1);

    if won {
        let response = next.run(req).await;
        let status = response.status().as_u16();
        let (parts, body) = response.into_parts();
        let response_bytes = to_bytes(body, BODY_LIMIT).await.unwrap_or_default();

        let _ = sqlx::query(
            "UPDATE idempotency_keys SET state = 'complete', response_status = ?1, response_body = ?2
             WHERE merchant_id = ?3 AND endpoint = ?4 AND key = ?5",
        )
        .bind(status as i64)
        .bind(String::from_utf8_lossy(&response_bytes).to_string())
        .bind(merchant_id.to_string())
        .bind(&endpoint)
        .bind(&key)
        .execute(&state.pool)
        .await;

        return Response::from_parts(parts, Body::from(response_bytes));
    }

    let existing: Option<StoredResponse> = sqlx::query_as(
        "SELECT request_hash, state, response_status, response_body FROM idempotency_keys
         WHERE merchant_id = ?1 AND endpoint = ?2 AND key = ?3",
    )
    .bind(merchant_id.to_string())
    .bind(&endpoint)
    .bind(&key)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None)
    .map(
        |(request_hash, state, response_status, response_body): (
            String,
            String,
            Option<i64>,
            Option<String>,
        )| StoredResponse {
            request_hash,
            state,
            response_status,
            response_body,
        },
    );

    match existing {
        Some(row) if row.request_hash != hash => {
            AcmeError::Conflict("idempotency key reused with a different request body".into())
                .into_response()
        }
        Some(StoredResponse {
            state,
            response_status: Some(status),
            response_body: Some(body),
            ..
        }) if state == "complete" => Response::builder()
            .status(StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK))
            .header(header::CONTENT_TYPE, "application/json")
            .header("acme-idempotent-replay", HeaderValue::from_static("true"))
            .body(Body::from(body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Some(_) => AcmeError::Conflict(
            "request with this idempotency key is still in flight, retry shortly".into(),
        )
        .into_response(),
        // The insert lost the race for a reason other than a live
        // conflicting row (e.g. a transient sqlx error) — fail open rather
        // than block the request logging can't explain.
        None => next.run(req).await,
    }
}
