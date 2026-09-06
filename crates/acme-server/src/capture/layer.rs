//! Inbound capture middleware (spec §21.1-§21.6), mounted with
//! `axum::middleware::from_fn_with_state(CaptureState { .. }, record_exchange)`.
//! Buffers and redacts both bodies, writes one `Exchange` through the
//! `Recorder`, and stamps `Acme-Request-Id`/`Acme-Trace-Id` on the response.
//! Nothing under M1 mounts this yet — no provider router exists until M2 —
//! it's built and proven against a synthetic router (see the crate's
//! `tests/capture.rs`).

use std::sync::LazyLock;
use std::time::Instant;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use bytes::Bytes;
use chrono::Utc;
use regex::Regex;

use crate::capture::recorder::{BodyCapture, Channel, Exchange, Recorder, ResourceRef};
use crate::capture::{redact, trace};
use crate::domain::ids::{ExchangeId, TraceId};
use crate::sim::clock::SimClock;

/// Bodies over this size are read but not stored redacted in full — spec
/// §21.7's configurable limits (`ACME_TRAFFIC_BODY_LIMIT`) land with the
/// dashboard's capture-mode controls in M6; this is a fixed interim cap.
const BODY_READ_LIMIT: usize = 2 * 1024 * 1024;

static ID_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:pay|shp|cs|ord|mrc|evt|ref|dsp|qte|rto|pck|whe|whd)_[0-9A-Za-z]{26}\b")
        .expect("static search-key regex is valid")
});

#[derive(Clone)]
pub struct CaptureState {
    pub recorder: Recorder,
    pub clock: SimClock,
    pub channel: Channel,
}

/// W3C `traceparent` interop is out of scope until there's an external
/// system to interoperate with; only our own header round-trips.
fn incoming_trace_id(headers: &axum::http::HeaderMap) -> TraceId {
    headers
        .get("acme-trace-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<TraceId>().ok())
        .unwrap_or_else(TraceId::new)
}

fn capture_headers(headers: &axum::http::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = name.as_str().to_string();
            let raw = value.to_str().unwrap_or("<binary>").to_string();
            let value = if redact::is_sensitive_header(&name) {
                redact::redact_header_value(&raw)
            } else {
                raw
            };
            (name, value)
        })
        .collect()
}

fn content_type(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn body_capture(original: &[u8], content_type: Option<String>) -> Option<BodyCapture> {
    if original.is_empty() {
        return None;
    }
    let (redacted, redactions) = redact::redact_body(original);
    let encoding = if std::str::from_utf8(&redacted).is_ok() {
        "utf8"
    } else {
        "base64"
    };
    let bytes = if encoding == "utf8" {
        Bytes::from(redacted)
    } else {
        Bytes::from(
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &redacted)
                .into_bytes(),
        )
    };
    Some(BodyCapture {
        content_type,
        bytes,
        encoding,
        redactions,
    })
}

fn scan_ids(into: &mut Vec<String>, text: &str) {
    for m in ID_TOKEN.find_iter(text) {
        let token = m.as_str().to_string();
        if !into.contains(&token) {
            into.push(token);
        }
    }
}

fn build_search_key(
    path: &str,
    request_body: Option<&[u8]>,
    response_body: Option<&[u8]>,
) -> Option<String> {
    let mut found = Vec::new();
    scan_ids(&mut found, path);
    for body in [request_body, response_body].into_iter().flatten() {
        if let Ok(text) = std::str::from_utf8(body) {
            scan_ids(&mut found, text);
        }
    }
    (!found.is_empty()).then(|| found.join(" "))
}

fn outcome_for(status: u16) -> &'static str {
    match status {
        0..=399 => "ok",
        400..=499 => "client_error",
        _ => "server_error",
    }
}

/// `route_pattern` is always `None`: mounted via `Router::layer()` (not
/// `route_layer()`) so 404s are captured too, per spec §21.3 — but that
/// means this runs before the router's own routing step, which is what
/// would otherwise populate `MatchedPath`. See `AGENTS.md` Learnings.
pub async fn record_exchange(
    State(capture): State<CaptureState>,
    req: Request,
    next: Next,
) -> Response {
    let wall_start = Instant::now();
    let started_at = Utc::now();
    let sim_at = capture.clock.now();

    let method = req.method().to_string();
    let uri = req.uri().clone();
    let url = uri.to_string();
    let path = uri.path().to_string();
    let query = uri.query().map(str::to_string);
    let route_pattern: Option<String> = None;
    let request_headers = capture_headers(req.headers());
    let request_content_type = content_type(req.headers());

    let (parts, body) = req.into_parts();
    let raw_request_body = to_bytes(body, BODY_READ_LIMIT).await.unwrap_or_default();
    let request_body_capture = body_capture(&raw_request_body, request_content_type);
    let request_headers_for_trace = parts.headers.clone();
    let req = Request::from_parts(parts, Body::from(raw_request_body.clone()));

    let trace_id = incoming_trace_id(&request_headers_for_trace);
    let exchange_id = ExchangeId::new();

    // `take_resources` must run inside the same `scope` as `next.run` — the
    // task-local it reads is only set for the duration of that future.
    let (response, resources) = trace::scope(trace_id, async {
        let response = next.run(req).await;
        (response, trace::take_resources())
    })
    .await;

    let duration_ms = wall_start.elapsed().as_millis() as i64;
    let status = response.status().as_u16();
    let response_headers = capture_headers(response.headers());
    let response_content_type = content_type(response.headers());

    let (mut parts, body) = response.into_parts();
    let raw_response_body = to_bytes(body, BODY_READ_LIMIT).await.unwrap_or_default();
    let response_body_capture = body_capture(&raw_response_body, response_content_type);

    parts.headers.insert(
        "acme-request-id",
        HeaderValue::from_str(&exchange_id.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );
    parts.headers.insert(
        "acme-trace-id",
        HeaderValue::from_str(&trace_id.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );

    let search_key = build_search_key(
        &path,
        (!raw_request_body.is_empty()).then_some(raw_request_body.as_ref()),
        (!raw_response_body.is_empty()).then_some(raw_response_body.as_ref()),
    );
    let resources = resources
        .into_iter()
        .map(|r| ResourceRef {
            resource_type: r.resource_type.to_string(),
            resource_id: r.resource_id,
            role: r.role.to_string(),
        })
        .collect();

    capture.recorder.record(Exchange {
        id: exchange_id,
        trace_id,
        parent_id: None,
        direction: "inbound",
        channel: capture.channel,
        started_at,
        sim_at,
        duration_ms,
        method,
        url,
        path,
        query,
        route_pattern,
        request_headers,
        request_body: request_body_capture,
        status_code: Some(status),
        response_headers,
        response_body: response_body_capture,
        outcome: outcome_for(status),
        search_key,
        resources,
        delivery_id: None,
        event_id: None,
        attempt: None,
        signed_payload: None,
        signature: None,
    });

    Response::from_parts(parts, Body::from(raw_response_body))
}
