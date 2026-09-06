//! Bounded channel + single writer task (spec §21.6). `record` never
//! blocks and never fails the request that triggered it — a saturated
//! channel drops the exchange and increments a counter instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, SqlitePool, Transaction};
use tokio::sync::mpsc;

use crate::domain::ids::{EventId, ExchangeId, TraceId, WebhookDeliveryId};

const CHANNEL_CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Api,
    HostedPage,
    Dashboard,
    Webhook,
    Replay,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Api => "api",
            Channel::HostedPage => "hosted_page",
            Channel::Dashboard => "dashboard",
            Channel::Webhook => "webhook",
            Channel::Replay => "replay",
        }
    }
}

pub struct BodyCapture {
    pub content_type: Option<String>,
    pub bytes: Bytes,
    pub encoding: &'static str,
    pub redactions: u32,
}

pub struct ResourceRef {
    pub resource_type: String,
    pub resource_id: String,
    pub role: String,
}

/// One HTTP exchange, ready to be written to `http_exchanges`/`http_bodies`
/// (spec §21.2). Built by `capture::layer`, consumed by the writer task.
pub struct Exchange {
    pub id: ExchangeId,
    pub trace_id: TraceId,
    pub parent_id: Option<ExchangeId>,
    pub direction: &'static str,
    pub channel: Channel,
    pub started_at: DateTime<Utc>,
    pub sim_at: DateTime<Utc>,
    pub duration_ms: i64,
    pub method: String,
    pub url: String,
    pub path: String,
    pub query: Option<String>,
    pub route_pattern: Option<String>,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<BodyCapture>,
    pub status_code: Option<u16>,
    pub response_headers: Vec<(String, String)>,
    pub response_body: Option<BodyCapture>,
    pub outcome: &'static str,
    pub search_key: Option<String>,
    pub resources: Vec<ResourceRef>,
    /// Set only for `Channel::Webhook` attempts (spec §21.9/§21.14's
    /// M4 delta): links this exchange back to the delivery/event it
    /// belongs to and records the exact signed string and signature sent,
    /// so the signature pane never has to recompute either.
    pub delivery_id: Option<WebhookDeliveryId>,
    pub event_id: Option<EventId>,
    pub attempt: Option<i32>,
    pub signed_payload: Option<String>,
    pub signature: Option<String>,
}

#[derive(Clone)]
pub struct Recorder {
    tx: mpsc::Sender<Exchange>,
    dropped: Arc<AtomicU64>,
}

impl Recorder {
    /// Spawns the writer task and returns a handle plus its join handle.
    pub fn spawn(pool: SqlitePool) -> (Self, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let handle = tokio::spawn(writer_loop(pool, rx));
        (Self { tx, dropped }, handle)
    }

    /// Never blocks, never fails the caller. Drops with a counter increment
    /// when the channel is saturated (spec §21.13 acceptance criterion 6).
    pub fn record(&self, exchange: Exchange) {
        if self.tx.try_send(exchange).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

async fn writer_loop(pool: SqlitePool, mut rx: mpsc::Receiver<Exchange>) {
    while let Some(exchange) = rx.recv().await {
        if let Err(error) = write_exchange(&pool, exchange).await {
            tracing::error!(?error, "capture: failed to write exchange");
        }
    }
}

fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

async fn upsert_body(
    tx: &mut Transaction<'_, Sqlite>,
    body: &BodyCapture,
) -> anyhow::Result<String> {
    let id = content_hash(&body.bytes);

    sqlx::query(
        "INSERT INTO http_bodies (id, content_type, encoding, size_bytes, stored_bytes, truncated, redactions, body, refcount, created_at)
         VALUES (?1, ?2, ?3, ?4, ?4, 0, ?5, ?6, 1, ?7)
         ON CONFLICT(id) DO UPDATE SET refcount = refcount + 1",
    )
    .bind(&id)
    .bind(&body.content_type)
    .bind(body.encoding)
    .bind(body.bytes.len() as i64)
    .bind(body.redactions as i64)
    .bind(body.bytes.as_ref())
    .bind(Utc::now().to_rfc3339())
    .execute(&mut **tx)
    .await?;

    Ok(id)
}

/// `pub(crate)` rather than private so `db::repo::traffic`'s tests can
/// write a fixture exchange without going through the async channel.
pub(crate) async fn write_exchange(pool: &SqlitePool, exchange: Exchange) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;

    let request_body_id = match &exchange.request_body {
        Some(body) => Some(upsert_body(&mut tx, body).await?),
        None => None,
    };
    let response_body_id = match &exchange.response_body {
        Some(body) => Some(upsert_body(&mut tx, body).await?),
        None => None,
    };

    let request_bytes = exchange
        .request_body
        .as_ref()
        .map(|b| b.bytes.len() as i64)
        .unwrap_or(0);
    let response_bytes = exchange
        .response_body
        .as_ref()
        .map(|b| b.bytes.len() as i64)
        .unwrap_or(0);
    let request_headers_json = serde_json::to_string(&exchange.request_headers)?;
    let response_headers_json = serde_json::to_string(&exchange.response_headers)?;

    sqlx::query(
        "INSERT INTO http_exchanges (
            id, trace_id, parent_id, direction, channel, started_at, sim_at, duration_ms,
            method, url, path, query, route_pattern,
            request_headers, request_body_id, request_bytes,
            status_code, response_headers, response_body_id, response_bytes,
            outcome, search_key,
            delivery_id, event_id, attempt, signed_payload, signature
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
            ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16,
            ?17, ?18, ?19, ?20,
            ?21, ?22,
            ?23, ?24, ?25, ?26, ?27
        )",
    )
    .bind(exchange.id.to_string())
    .bind(exchange.trace_id.to_string())
    .bind(exchange.parent_id.map(|p| p.to_string()))
    .bind(exchange.direction)
    .bind(exchange.channel.as_str())
    .bind(exchange.started_at.to_rfc3339())
    .bind(exchange.sim_at.to_rfc3339())
    .bind(exchange.duration_ms)
    .bind(&exchange.method)
    .bind(&exchange.url)
    .bind(&exchange.path)
    .bind(&exchange.query)
    .bind(&exchange.route_pattern)
    .bind(request_headers_json)
    .bind(&request_body_id)
    .bind(request_bytes)
    .bind(exchange.status_code.map(|s| s as i64))
    .bind(response_headers_json)
    .bind(&response_body_id)
    .bind(response_bytes)
    .bind(exchange.outcome)
    .bind(&exchange.search_key)
    .bind(exchange.delivery_id.map(|d| d.to_string()))
    .bind(exchange.event_id.map(|e| e.to_string()))
    .bind(exchange.attempt)
    .bind(&exchange.signed_payload)
    .bind(&exchange.signature)
    .execute(&mut *tx)
    .await?;

    for resource in &exchange.resources {
        sqlx::query(
            "INSERT INTO exchange_resources (exchange_id, resource_type, resource_id, role)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(exchange.id.to_string())
        .bind(&resource.resource_type)
        .bind(&resource.resource_id)
        .bind(&resource.role)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    crate::dashboard::activity::publish(crate::dashboard::activity::ActivityEvent::http(
        format!(
            "{} {} {} {}ms",
            exchange.direction, exchange.method, exchange.path, exchange.duration_ms
        ),
        exchange.sim_at,
    ));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_exchange() -> Exchange {
        Exchange {
            id: ExchangeId::new(),
            trace_id: TraceId::new(),
            parent_id: None,
            direction: "inbound",
            channel: Channel::Api,
            started_at: Utc::now(),
            sim_at: Utc::now(),
            duration_ms: 0,
            method: "GET".into(),
            url: "/x".into(),
            path: "/x".into(),
            query: None,
            route_pattern: None,
            request_headers: vec![],
            request_body: None,
            status_code: Some(200),
            response_headers: vec![],
            response_body: None,
            outcome: "ok",
            search_key: None,
            resources: vec![],
            delivery_id: None,
            event_id: None,
            attempt: None,
            signed_payload: None,
            signature: None,
        }
    }

    /// §21.13 acceptance criterion 6: filling the channel never blocks the
    /// caller, and the drop is counted. No writer task is spawned here —
    /// the receiver is simply never drained.
    #[test]
    fn record_never_blocks_and_counts_drops_when_saturated() {
        let (tx, _rx) = mpsc::channel(1);
        let recorder = Recorder {
            tx,
            dropped: Arc::new(AtomicU64::new(0)),
        };

        recorder.record(dummy_exchange());
        recorder.record(dummy_exchange());

        assert_eq!(recorder.dropped_count(), 1);
    }
}
