//! Outbound webhook delivery (spec §12.1, specs/005-Webhooks.md). Polls due
//! deliveries every wall-clock second — mirroring `sim::ticker`'s shape —
//! and is also the entry point the dashboard's manual retry/resend
//! mutations call for an out-of-schedule attempt.

use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use crate::capture::recorder::{BodyCapture, Channel, Exchange};
use crate::dashboard::activity::{self, ActivityEvent};
use crate::db::repo::webhooks::{self, AttemptOutcome, DueDelivery};
use crate::domain::ids::ExchangeId;
use crate::domain::webhook::SigningScheme;
use crate::state::AppState;

/// Spawns the dispatcher loop; cancels cleanly on `token.cancel()`. Gated
/// on `cfg.webhooks_enabled` by the caller (`lib.rs`) — the fan-out step
/// (`db::repo::payments`/`shipments`) always inserts `pending` deliveries
/// regardless, they just never get attempted if this never runs.
pub fn spawn(state: AppState, token: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(StdDuration::from_secs(1));
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = tick(&state).await {
                        tracing::error!(?error, "webhook dispatcher: tick failed");
                    }
                }
            }
        }
    })
}

async fn tick(state: &AppState) -> anyhow::Result<usize> {
    let now = state.clock.now();
    let due = webhooks::due(&state.db, now, 50).await?;
    let count = due.len();
    for delivery in due {
        attempt_delivery(state, &delivery, now).await;
    }
    Ok(count)
}

fn outcome_for(status: Option<u16>) -> (&'static str, bool) {
    match status {
        Some(code) if (200..300).contains(&code) => ("ok", true),
        Some(code) if (400..500).contains(&code) => ("client_error", false),
        Some(_) => ("server_error", false),
        None => ("error", false),
    }
}

/// A fresh attempt: new timestamp, new signature, same stored payload
/// bytes — the dispatcher's normal tick and the dashboard's "Retry now"
/// mutation both call this.
pub async fn attempt_delivery(state: &AppState, delivery: &DueDelivery, now: DateTime<Utc>) {
    let attempt_number = delivery.attempt + 1;

    let body_bytes = match webhooks::read_payload_body(&state.db, &delivery.payload_body_id).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(?error, delivery_id = %delivery.id, "webhook dispatcher: failed to read payload body");
            return;
        }
    };

    let scheme = SigningScheme::for_provider(&delivery.provider_slug);
    let signed = scheme.sign(&delivery.endpoint_secret, now.timestamp(), &body_bytes);

    send_and_record(
        state,
        delivery,
        attempt_number,
        &body_bytes,
        scheme,
        &signed.header_value,
        &signed.signed_string,
        &signed.signature,
        now,
    )
    .await;
}

/// Byte-identical replay: reuses the *original* timestamp and signature
/// from the delivery's most recent attempt rather than recomputing either
/// (spec §21.9's "Resend as originally signed" — "how you test a
/// receiver's timestamp tolerance and replay protection").
pub async fn resend_delivery(state: &AppState, delivery: &DueDelivery, now: DateTime<Utc>) {
    let attempt_number = delivery.attempt + 1;

    let body_bytes = match webhooks::read_payload_body(&state.db, &delivery.payload_body_id).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(?error, delivery_id = %delivery.id, "webhook dispatcher: failed to read payload body");
            return;
        }
    };

    let last = match crate::db::repo::traffic::list_by_delivery(&state.db, delivery.id).await {
        Ok(mut attempts) => attempts.pop(),
        Err(error) => {
            tracing::error!(?error, delivery_id = %delivery.id, "webhook dispatcher: failed to load prior attempts");
            return;
        }
    };
    let Some((signed_payload, signature)) =
        last.and_then(|a| Some((a.signed_payload?, a.signature?)))
    else {
        tracing::warn!(delivery_id = %delivery.id, "webhook dispatcher: no prior signed attempt to resend");
        return;
    };
    let Some(timestamp) = signed_payload
        .split_once('.')
        .and_then(|(t, _)| t.parse::<i64>().ok())
    else {
        tracing::error!(delivery_id = %delivery.id, "webhook dispatcher: could not parse original timestamp");
        return;
    };

    let scheme = SigningScheme::for_provider(&delivery.provider_slug);
    let header_value = scheme.header_value_for(timestamp, &signature);

    send_and_record(
        state,
        delivery,
        attempt_number,
        &body_bytes,
        scheme,
        &header_value,
        &signed_payload,
        &signature,
        now,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn send_and_record(
    state: &AppState,
    delivery: &DueDelivery,
    attempt_number: i64,
    body_bytes: &[u8],
    scheme: SigningScheme,
    header_value: &str,
    signed_payload: &str,
    signature: &str,
    now: DateTime<Utc>,
) {
    let wall_start = Instant::now();
    let started_at = Utc::now();
    let sim_at = state.clock.now();
    let exchange_id = ExchangeId::new();

    let request_headers = vec![
        (scheme.header_name().to_string(), header_value.to_string()),
        (
            "Acme-Webhook-Id".to_string(),
            delivery.endpoint_id.to_string(),
        ),
        ("Acme-Event-Id".to_string(), delivery.event_id.to_string()),
        ("Acme-Attempt".to_string(), attempt_number.to_string()),
        (
            "User-Agent".to_string(),
            format!("Acme/1.0 (+webhooks; provider={})", delivery.provider_slug),
        ),
        ("Content-Type".to_string(), "application/json".to_string()),
    ];

    let mut request = state.http_client.post(&delivery.endpoint_url);
    for (name, value) in &request_headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let request = request
        .timeout(StdDuration::from_millis(state.cfg.webhook_timeout_ms))
        .body(body_bytes.to_vec());

    // Simulator's `webhook_failure_rate` slider (spec §9.3,
    // specs/008-Simulation.md item 2) — rolled right before the real
    // network call, treated exactly like a genuine connection failure
    // below, so the retry/exhaustion/auto-disable machinery
    // specs/005-Webhooks.md already built doesn't need a third code path.
    let simulated_failure = crate::sim::fault::webhook_delivery_should_fail(&state.db).await;
    let result = if simulated_failure {
        None
    } else {
        Some(request.send().await)
    };
    let duration_ms = wall_start.elapsed().as_millis() as i64;

    let (status_code, response_headers, response_body_bytes) = match result {
        None => (None, Vec::new(), Vec::new()),
        Some(Ok(response)) => {
            let status = response.status().as_u16();
            let headers: Vec<(String, String)> = response
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        v.to_str().unwrap_or("<binary>").to_string(),
                    )
                })
                .collect();
            let bytes = response.bytes().await.unwrap_or_default().to_vec();
            (Some(status), headers, bytes)
        }
        Some(Err(_error)) => (None, Vec::new(), Vec::new()),
    };
    let (outcome_str, success) = outcome_for(status_code);

    let response_body = (!response_body_bytes.is_empty()).then(|| {
        let encoding = if std::str::from_utf8(&response_body_bytes).is_ok() {
            "utf8"
        } else {
            "base64"
        };
        BodyCapture {
            content_type: None,
            bytes: response_body_bytes.into(),
            encoding,
            redactions: 0,
        }
    });

    let exchange = Exchange {
        id: exchange_id,
        trace_id: delivery.trace_id,
        parent_id: None,
        direction: "outbound",
        channel: Channel::Webhook,
        started_at,
        sim_at,
        duration_ms,
        method: "POST".to_string(),
        url: delivery.endpoint_url.clone(),
        path: delivery.endpoint_url.clone(),
        query: None,
        route_pattern: None,
        request_headers,
        request_body: Some(BodyCapture {
            content_type: Some("application/json".to_string()),
            bytes: body_bytes.to_vec().into(),
            encoding: "utf8",
            redactions: 0,
        }),
        status_code,
        response_headers,
        response_body,
        outcome: outcome_str,
        search_key: None,
        resources: vec![],
        delivery_id: Some(delivery.id),
        event_id: Some(delivery.event_id),
        attempt: Some(attempt_number as i32),
        signed_payload: Some(signed_payload.to_string()),
        signature: Some(signature.to_string()),
    };

    // Written synchronously (not via `Recorder::record`'s async channel):
    // `record_attempt` below sets `webhook_deliveries.last_exchange_id`,
    // which is a foreign key into `http_exchanges` — it must already exist
    // by the time that write happens, or the FK check fails (spec's
    // `PRAGMA foreign_keys = ON`, `db::mod`).
    if let Err(error) = crate::capture::recorder::write_exchange(&state.db, exchange).await {
        tracing::error!(?error, delivery_id = %delivery.id, "webhook dispatcher: failed to record exchange");
        return;
    }

    let outcome = AttemptOutcome {
        success,
        exchange_id,
    };
    match webhooks::record_attempt(
        &state.db,
        delivery.id,
        delivery.endpoint_id,
        attempt_number,
        delivery.max_attempts,
        &outcome,
        now,
    )
    .await
    {
        Ok(auto_disabled) => {
            let verb = if success { "succeeded" } else { "failed" };
            activity::publish(ActivityEvent::resource(
                "webhook",
                delivery.id.to_string(),
                format!(
                    "{} \u{2192} {} attempt {attempt_number} {verb}",
                    delivery.event_type, delivery.endpoint_id
                ),
                sim_at,
            ));
            if auto_disabled {
                tracing::warn!(endpoint_id = %delivery.endpoint_id, "webhook endpoint auto-disabled after 20 consecutive failures");
            }
        }
        Err(error) => {
            tracing::error!(?error, delivery_id = %delivery.id, "webhook dispatcher: failed to record attempt");
        }
    }
}
