//! Fault injection (spec §9.3, specs/008-Simulation.md item 1): one
//! middleware, mounted per dialect exactly where `capture::layer` already
//! is, one layer further in so an injected failure is still captured.
//!
//! Modes implemented: `error`, `latency`, `malformed`, `rate_limit`, and
//! `timeout` (approximated as a long delay followed by `504` — see the
//! module's own Caveats in specs/008-Simulation.md for why a literal
//! never-respond isn't implemented).

use std::time::Duration as StdDuration;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rand::Rng;
use sqlx::SqlitePool;

use crate::db::repo::faults::{self, FaultRow};
use crate::db::repo::sim as sim_settings;
use crate::sim::clock::SimClock;

/// A long delay standing in for "hold the connection past the client's
/// patience" (spec §9.3's `timeout` mode) — see this module's doc
/// comment for why a true indefinite hang isn't implemented.
const TIMEOUT_MODE_DELAY_MS: u64 = 5_000;
const BODY_READ_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct FaultState {
    pub pool: SqlitePool,
    pub clock: SimClock,
    pub provider_slug: &'static str,
}

fn error_response(status: StatusCode, code: &str, message: String) -> Response {
    let body = serde_json::json!({
        "error": {
            "type": code,
            "code": code,
            "message": message,
            "param": serde_json::Value::Null,
            "request_id": serde_json::Value::Null,
        }
    });
    (status, axum::Json(body)).into_response()
}

/// A single `*` wildcard, matching any run of characters (spec §9.3's own
/// example, `/chargeflow/v1/checkout/sessions`, needs nothing richer). No
/// glob crate — splitting on `*` and checking each literal segment shows
/// up, in order, covers every documented fault rule.
fn glob_match(glob: &str, path: &str) -> bool {
    let Some((first, rest)) = glob.split_once('*') else {
        return glob == path;
    };
    let Some(mut remaining) = path.strip_prefix(first) else {
        return false;
    };
    let mut rest = rest;
    loop {
        match rest.split_once('*') {
            Some((segment, tail)) => {
                let Some(idx) = remaining.find(segment) else {
                    return false;
                };
                remaining = &remaining[idx + segment.len()..];
                rest = tail;
            }
            None => return remaining.ends_with(rest),
        }
    }
}

fn roll(probability: f64) -> bool {
    probability >= 1.0 || rand::thread_rng().gen_range(0.0..1.0) < probability.max(0.0)
}

pub async fn inject(State(state): State<FaultState>, req: Request, next: Next) -> Response {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();

    // Global baseline (spec §9.3/§13.6's sliders), independent of any
    // configured `sim_faults` row.
    if let Ok(settings) = sim_settings::get(&state.pool).await {
        if settings.latency_ms > 0 {
            tracing::debug!(
                provider_slug = state.provider_slug,
                method,
                path,
                latency_ms = settings.latency_ms,
                "fault inject: applying global latency slider"
            );
            tokio::time::sleep(StdDuration::from_millis(settings.latency_ms as u64)).await;
        }
        if settings.failure_rate > 0.0 && roll(settings.failure_rate) {
            tracing::info!(
                provider_slug = state.provider_slug,
                method,
                path,
                failure_rate = settings.failure_rate,
                "fault inject: global failure slider fired, returning 503"
            );
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "provider_down",
                "simulated global failure (Simulator failure slider)".to_string(),
            );
        }
    }

    let candidates = faults::list_active_candidates(
        &state.pool,
        state.provider_slug,
        &method,
        state.clock.now(),
    )
    .await
    .unwrap_or_default();

    for fault in candidates {
        if !glob_match(&fault.path_glob, &path) {
            continue;
        }
        if !roll(fault.probability) {
            continue;
        }

        tracing::info!(provider_slug = state.provider_slug, method, path, fault_id = %fault.id, mode = %fault.mode, "fault inject: configured fault rule fired");
        let _ = faults::decrement_remaining(&state.pool, fault.id).await;
        return apply_fault(&fault, req, next).await;
    }

    next.run(req).await
}

async fn apply_fault(fault: &FaultRow, req: Request, next: Next) -> Response {
    match fault.mode.as_str() {
        "error" => {
            let status = fault
                .http_status
                .and_then(|s| u16::try_from(s).ok())
                .and_then(|s| StatusCode::from_u16(s).ok())
                .unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
            let code = fault
                .error_code
                .clone()
                .unwrap_or_else(|| "fault_injected".to_string());
            error_response(status, &code, format!("fault {} injected", fault.id))
        }
        "latency" => {
            let ms = fault.latency_ms.unwrap_or(0).max(0) as u64;
            tokio::time::sleep(StdDuration::from_millis(ms)).await;
            next.run(req).await
        }
        "timeout" => {
            tokio::time::sleep(StdDuration::from_millis(TIMEOUT_MODE_DELAY_MS)).await;
            error_response(
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                format!("fault {} injected: simulated timeout", fault.id),
            )
        }
        "malformed" => {
            let response = next.run(req).await;
            let (parts, body) = response.into_parts();
            let bytes = to_bytes(body, BODY_READ_LIMIT).await.unwrap_or_default();
            let cut = (bytes.len() / 2).max(1).min(bytes.len());
            Response::from_parts(parts, Body::from(bytes.slice(0..cut)))
        }
        "rate_limit" => {
            let mut response = error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                format!("fault {} injected: simulated rate limit", fault.id),
            );
            response
                .headers_mut()
                .insert("retry-after", HeaderValue::from_static("30"));
            response
        }
        _ => next.run(req).await,
    }
}

/// `dashboard::dispatcher`'s own equivalent roll — see
/// specs/008-Simulation.md item 2: `webhook_failure_rate` is applied
/// right before the real outbound `reqwest` call, not through this
/// inbound-request middleware.
pub async fn webhook_delivery_should_fail(pool: &SqlitePool) -> bool {
    let Ok(settings) = sim_settings::get(pool).await else {
        return false;
    };
    let should_fail = settings.webhook_failure_rate > 0.0 && roll(settings.webhook_failure_rate);
    if should_fail {
        tracing::info!(
            webhook_failure_rate = settings.webhook_failure_rate,
            "fault inject: webhook failure slider fired, delivery will be simulated as failed"
        );
    }
    should_fail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_prefix_wildcard_suffix() {
        assert!(glob_match(
            "/chargeflow/v1/checkout/sessions",
            "/chargeflow/v1/checkout/sessions"
        ));
        assert!(glob_match("/acmepay/v1/*", "/acmepay/v1/payments"));
        assert!(glob_match("/acmepay/v1/*", "/acmepay/v1/payments/pay_123"));
        assert!(!glob_match("/acmepay/v1/*", "/acmeship/v1/shipments"));
        assert!(glob_match(
            "*/refunds",
            "/acmepay/v1/payments/pay_1/refunds"
        ));
        assert!(!glob_match("*/refunds", "/acmepay/v1/payments/pay_1"));
    }

    #[test]
    fn probability_at_or_above_one_always_fires() {
        for _ in 0..20 {
            assert!(roll(1.0));
        }
    }

    #[test]
    fn probability_zero_fires_rarely_over_many_trials() {
        let fired = (0..1000).filter(|_| roll(0.0)).count();
        assert_eq!(fired, 0, "probability 0.0 must never fire");
    }
}
