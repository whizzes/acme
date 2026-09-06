//! Webhook endpoint registration (spec §7.5, §10.1/§11.1's `webhook_endpoints`
//! CRUD), plus fan-out and dispatch queries backing `dashboard::dispatcher`
//! (spec §12.1, specs/005-Webhooks.md).

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::domain::ids::{
    EventId, ExchangeId, MerchantId, TraceId, WebhookDeliveryId, WebhookEndpointId,
};

pub struct NewWebhookEndpoint {
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub url: String,
    pub secret: String,
    pub enabled_events: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct WebhookEndpointRow {
    pub id: WebhookEndpointId,
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub url: String,
    pub enabled_events: String,
    pub active: bool,
    pub consecutive_failures: i64,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct WebhookEndpointSqlRow {
    id: String,
    merchant_id: String,
    provider_slug: String,
    url: String,
    enabled_events: String,
    active: i64,
    consecutive_failures: i64,
    description: Option<String>,
    created_at: String,
}

const ENDPOINT_COLUMNS: &str = "id, merchant_id, provider_slug, url, enabled_events, active, consecutive_failures, description, created_at";

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .expect("stored timestamps are valid rfc3339")
        .with_timezone(&Utc)
}

impl WebhookEndpointSqlRow {
    fn into_row(self) -> WebhookEndpointRow {
        WebhookEndpointRow {
            id: self
                .id
                .parse()
                .expect("stored webhook endpoint id is valid"),
            merchant_id: self
                .merchant_id
                .parse()
                .expect("stored merchant id is valid"),
            provider_slug: self.provider_slug,
            url: self.url,
            enabled_events: self.enabled_events,
            active: self.active != 0,
            consecutive_failures: self.consecutive_failures,
            description: self.description,
            created_at: parse_dt(&self.created_at),
        }
    }
}

pub async fn create(
    pool: &SqlitePool,
    new: &NewWebhookEndpoint,
) -> anyhow::Result<WebhookEndpointId> {
    let id = WebhookEndpointId::new();
    sqlx::query(
        "INSERT INTO webhook_endpoints (id, merchant_id, provider_slug, url, secret, enabled_events, active, description, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8)",
    )
    .bind(id.to_string())
    .bind(new.merchant_id.to_string())
    .bind(&new.provider_slug)
    .bind(&new.url)
    .bind(&new.secret)
    .bind(&new.enabled_events)
    .bind(&new.description)
    .bind(new.created_at.to_rfc3339())
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn get(
    pool: &SqlitePool,
    merchant_id: MerchantId,
    id: WebhookEndpointId,
) -> anyhow::Result<Option<WebhookEndpointRow>> {
    let row: Option<WebhookEndpointSqlRow> = sqlx::query_as(&format!(
        "SELECT {ENDPOINT_COLUMNS} FROM webhook_endpoints WHERE id = ?1 AND merchant_id = ?2"
    ))
    .bind(id.to_string())
    .bind(merchant_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(WebhookEndpointSqlRow::into_row))
}

/// Unscoped by merchant — the dashboard has no auth (spec §18), same
/// reasoning as `db::repo::payments::ListFilter`'s optional scoping.
pub async fn get_unscoped(
    pool: &SqlitePool,
    id: WebhookEndpointId,
) -> anyhow::Result<Option<WebhookEndpointRow>> {
    let row: Option<WebhookEndpointSqlRow> = sqlx::query_as(&format!(
        "SELECT {ENDPOINT_COLUMNS} FROM webhook_endpoints WHERE id = ?1"
    ))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(WebhookEndpointSqlRow::into_row))
}

pub async fn delete(
    pool: &SqlitePool,
    merchant_id: MerchantId,
    id: WebhookEndpointId,
) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM webhook_endpoints WHERE id = ?1 AND merchant_id = ?2")
        .bind(id.to_string())
        .bind(merchant_id.to_string())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Dashboard list (spec §12.4's endpoint health): per endpoint, a coarse
/// success/failure count across every delivery it's ever had. `/webhooks`
/// renders these as the health bar; a finer 24-sim-hour bucketed bar is
/// left to a later polish pass (specs/005-Webhooks.md trims §13.6's page
/// detail scope to keep M4 to its own budget).
pub struct EndpointSummary {
    pub id: WebhookEndpointId,
    pub provider_slug: String,
    pub url: String,
    pub active: bool,
    pub consecutive_failures: i64,
    pub succeeded_count: i64,
    pub failed_count: i64,
}

pub async fn list_endpoints(pool: &SqlitePool) -> anyhow::Result<Vec<EndpointSummary>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        provider_slug: String,
        url: String,
        active: i64,
        consecutive_failures: i64,
        succeeded_count: i64,
        failed_count: i64,
    }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT we.id, we.provider_slug, we.url, we.active, we.consecutive_failures,
                COALESCE(SUM(CASE WHEN wd.status = 'succeeded' THEN 1 ELSE 0 END), 0) AS succeeded_count,
                COALESCE(SUM(CASE WHEN wd.status IN ('failed','exhausted') THEN 1 ELSE 0 END), 0) AS failed_count
         FROM webhook_endpoints we
         LEFT JOIN webhook_deliveries wd ON wd.endpoint_id = we.id
         GROUP BY we.id
         ORDER BY we.created_at DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(EndpointSummary {
                id: r.id.parse().ok()?,
                provider_slug: r.provider_slug,
                url: r.url,
                active: r.active != 0,
                consecutive_failures: r.consecutive_failures,
                succeeded_count: r.succeeded_count,
                failed_count: r.failed_count,
            })
        })
        .collect())
}

pub struct DeliveryRow {
    pub id: WebhookDeliveryId,
    pub event_type: String,
    pub status: String,
    pub attempt: i64,
    pub max_attempts: i64,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

pub async fn list_deliveries_for_endpoint(
    pool: &SqlitePool,
    endpoint_id: WebhookEndpointId,
) -> anyhow::Result<Vec<DeliveryRow>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        event_type: String,
        status: String,
        attempt: i64,
        max_attempts: i64,
        next_attempt_at: Option<String>,
        created_at: String,
        delivered_at: Option<String>,
    }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, event_type, status, attempt, max_attempts, next_attempt_at, created_at, delivered_at
         FROM webhook_deliveries WHERE endpoint_id = ?1 ORDER BY created_at DESC LIMIT 100",
    )
    .bind(endpoint_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(DeliveryRow {
                id: r.id.parse().ok()?,
                event_type: r.event_type,
                status: r.status,
                attempt: r.attempt,
                max_attempts: r.max_attempts,
                next_attempt_at: r.next_attempt_at.as_deref().map(parse_dt),
                created_at: parse_dt(&r.created_at),
                delivered_at: r.delivered_at.as_deref().map(parse_dt),
            })
        })
        .collect())
}

/// One delivery joined with its endpoint's `url`/`secret`/`provider_slug`
/// — everything `web::pages::webhooks`' signature pane and
/// `dashboard::dispatcher`'s manual retry/resend need in one query.
pub struct DeliveryDetail {
    pub id: WebhookDeliveryId,
    pub endpoint_id: WebhookEndpointId,
    pub endpoint_url: String,
    pub endpoint_secret: String,
    pub provider_slug: String,
    pub event_id: EventId,
    pub event_type: String,
    pub trace_id: TraceId,
    pub payload_body_id: Option<String>,
    pub attempt: i64,
    pub max_attempts: i64,
    pub status: String,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

pub async fn get_delivery(
    pool: &SqlitePool,
    id: WebhookDeliveryId,
) -> anyhow::Result<Option<DeliveryDetail>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        endpoint_id: String,
        endpoint_url: String,
        endpoint_secret: String,
        provider_slug: String,
        event_id: String,
        event_type: String,
        trace_id: String,
        payload_body_id: Option<String>,
        attempt: i64,
        max_attempts: i64,
        status: String,
        next_attempt_at: Option<String>,
        created_at: String,
        delivered_at: Option<String>,
    }

    let row: Option<Row> = sqlx::query_as(
        "SELECT wd.id, wd.endpoint_id, we.url AS endpoint_url, we.secret AS endpoint_secret, we.provider_slug,
                wd.event_id, wd.event_type, wd.trace_id, wd.payload_body_id, wd.attempt, wd.max_attempts,
                wd.status, wd.next_attempt_at, wd.created_at, wd.delivered_at
         FROM webhook_deliveries wd
         JOIN webhook_endpoints we ON we.id = wd.endpoint_id
         WHERE wd.id = ?1",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;

    Ok(row.and_then(|r| {
        Some(DeliveryDetail {
            id: r.id.parse().ok()?,
            endpoint_id: r.endpoint_id.parse().ok()?,
            endpoint_url: r.endpoint_url,
            endpoint_secret: r.endpoint_secret,
            provider_slug: r.provider_slug,
            event_id: r.event_id.parse().ok()?,
            event_type: r.event_type,
            trace_id: r.trace_id.parse().ok()?,
            payload_body_id: r.payload_body_id,
            attempt: r.attempt,
            max_attempts: r.max_attempts,
            status: r.status,
            next_attempt_at: r.next_attempt_at.as_deref().map(parse_dt),
            created_at: parse_dt(&r.created_at),
            delivered_at: r.delivered_at.as_deref().map(parse_dt),
        })
    }))
}

/// Manual enable/disable (spec §13.3's `POST /webhooks/endpoints/{id}/toggle`),
/// independent of the auto-disable counter — which re-enabling also resets,
/// per specs/005-Webhooks.md item 8.
pub async fn set_active(
    pool: &SqlitePool,
    id: WebhookEndpointId,
    active: bool,
) -> anyhow::Result<()> {
    if active {
        sqlx::query(
            "UPDATE webhook_endpoints SET active = 1, consecutive_failures = 0 WHERE id = ?1",
        )
        .bind(id.to_string())
        .execute(pool)
        .await?;
    } else {
        sqlx::query("UPDATE webhook_endpoints SET active = 0 WHERE id = ?1")
            .bind(id.to_string())
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn upsert_payload_body(pool: &SqlitePool, bytes: &[u8]) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let id = hex::encode(hasher.finalize());

    sqlx::query(
        "INSERT INTO http_bodies (id, content_type, encoding, size_bytes, stored_bytes, truncated, redactions, body, refcount, created_at)
         VALUES (?1, 'application/json', 'utf8', ?2, ?2, 0, 0, ?3, 1, ?4)
         ON CONFLICT(id) DO UPDATE SET refcount = refcount + 1",
    )
    .bind(&id)
    .bind(bytes.len() as i64)
    .bind(bytes)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;

    Ok(id)
}

/// Reads a payload's exact bytes back — every attempt of a delivery (and
/// every "Resend as originally signed") signs and sends the same bytes,
/// referenced by `webhook_deliveries.payload_body_id` (spec §21.9).
pub async fn read_payload_body(pool: &SqlitePool, body_id: &str) -> anyhow::Result<Vec<u8>> {
    let row: (Option<Vec<u8>>,) = sqlx::query_as("SELECT body FROM http_bodies WHERE id = ?1")
        .bind(body_id)
        .fetch_one(pool)
        .await?;
    Ok(row.0.unwrap_or_default())
}

/// One canonical event, ready to fan out to whichever endpoints match its
/// `provider_slug`/`event_type` (spec §12.1's "fan-out to matching
/// webhook_endpoints"). `envelope` is the full Stripe-shaped body
/// (`domain::webhook::build_envelope`) — built once here so every matching
/// endpoint's delivery shares the same `payload_body_id`.
pub struct FanOutEvent {
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub event_id: EventId,
    pub event_type: String,
    pub trace_id: TraceId,
    pub envelope: serde_json::Value,
    pub now: DateTime<Utc>,
}

pub async fn fan_out(
    pool: &SqlitePool,
    event: &FanOutEvent,
) -> anyhow::Result<Vec<WebhookDeliveryId>> {
    let candidates: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, enabled_events FROM webhook_endpoints
         WHERE merchant_id = ?1 AND provider_slug = ?2 AND active = 1",
    )
    .bind(event.merchant_id.to_string())
    .bind(&event.provider_slug)
    .fetch_all(pool)
    .await?;

    let mut created = Vec::new();
    let matching: Vec<String> = candidates
        .into_iter()
        .filter(|(_, enabled_events_json)| {
            let enabled: Vec<String> =
                serde_json::from_str(enabled_events_json).unwrap_or_default();
            enabled.iter().any(|e| e == "*") || enabled.iter().any(|e| e == &event.event_type)
        })
        .map(|(id, _)| id)
        .collect();

    if matching.is_empty() {
        return Ok(created);
    }

    let body_bytes = serde_json::to_vec(&event.envelope)?;
    let payload_body_id = upsert_payload_body(pool, &body_bytes).await?;
    let max_attempts = crate::domain::webhook::max_attempts();

    for endpoint_id in matching {
        let id = WebhookDeliveryId::new();
        sqlx::query(
            "INSERT INTO webhook_deliveries (id, endpoint_id, event_id, event_type, trace_id, payload_body_id, attempt, max_attempts, status, next_attempt_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, 'pending', ?8, ?9)",
        )
        .bind(id.to_string())
        .bind(&endpoint_id)
        .bind(event.event_id.to_string())
        .bind(&event.event_type)
        .bind(event.trace_id.to_string())
        .bind(&payload_body_id)
        .bind(max_attempts)
        .bind(event.now.to_rfc3339())
        .bind(event.now.to_rfc3339())
        .execute(pool)
        .await?;
        created.push(id);
    }

    Ok(created)
}

/// A delivery ready to attempt right now — either because `due()` found it,
/// or because a dashboard mutation is forcing an out-of-schedule attempt
/// (retry/resend, spec §13.3).
pub struct DueDelivery {
    pub id: WebhookDeliveryId,
    pub endpoint_id: WebhookEndpointId,
    pub endpoint_url: String,
    pub endpoint_secret: String,
    pub provider_slug: String,
    pub event_id: EventId,
    pub event_type: String,
    pub trace_id: TraceId,
    pub payload_body_id: String,
    pub attempt: i64,
    pub max_attempts: i64,
}

impl DueDelivery {
    /// `None` only if the delivery somehow has no `payload_body_id` yet —
    /// unreachable in practice since `fan_out` always sets one before the
    /// row exists, but the mutation handlers surface it as a 409 rather
    /// than unwrap.
    pub fn from_detail(detail: DeliveryDetail) -> Option<Self> {
        Some(Self {
            id: detail.id,
            endpoint_id: detail.endpoint_id,
            endpoint_url: detail.endpoint_url,
            endpoint_secret: detail.endpoint_secret,
            provider_slug: detail.provider_slug,
            event_id: detail.event_id,
            event_type: detail.event_type,
            trace_id: detail.trace_id,
            payload_body_id: detail.payload_body_id?,
            attempt: detail.attempt,
            max_attempts: detail.max_attempts,
        })
    }
}

/// Deliveries due now, at most one per endpoint per call (spec §12.1:
/// "deliveries to the same endpoint are serialized… a stuck endpoint does
/// not block other endpoints").
pub async fn due(
    pool: &SqlitePool,
    now: DateTime<Utc>,
    limit: i64,
) -> anyhow::Result<Vec<DueDelivery>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        endpoint_id: String,
        endpoint_url: String,
        endpoint_secret: String,
        provider_slug: String,
        event_id: String,
        event_type: String,
        trace_id: String,
        payload_body_id: Option<String>,
        attempt: i64,
        max_attempts: i64,
    }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT wd.id, wd.endpoint_id, we.url AS endpoint_url, we.secret AS endpoint_secret, we.provider_slug,
                wd.event_id, wd.event_type, wd.trace_id, wd.payload_body_id, wd.attempt, wd.max_attempts
         FROM webhook_deliveries wd
         JOIN webhook_endpoints we ON we.id = wd.endpoint_id
         WHERE we.active = 1
           AND wd.status IN ('pending', 'failed')
           AND wd.next_attempt_at IS NOT NULL AND wd.next_attempt_at <= ?1
           AND wd.id = (
               SELECT wd2.id FROM webhook_deliveries wd2
               WHERE wd2.endpoint_id = wd.endpoint_id
                 AND wd2.status IN ('pending', 'failed')
                 AND wd2.next_attempt_at IS NOT NULL AND wd2.next_attempt_at <= ?1
               ORDER BY wd2.next_attempt_at ASC, wd2.id ASC
               LIMIT 1
           )
         ORDER BY wd.next_attempt_at ASC
         LIMIT ?2",
    )
    .bind(now.to_rfc3339())
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(DueDelivery {
                id: r.id.parse().ok()?,
                endpoint_id: r.endpoint_id.parse().ok()?,
                endpoint_url: r.endpoint_url,
                endpoint_secret: r.endpoint_secret,
                provider_slug: r.provider_slug,
                event_id: r.event_id.parse().ok()?,
                event_type: r.event_type,
                trace_id: r.trace_id.parse().ok()?,
                payload_body_id: r.payload_body_id?,
                attempt: r.attempt,
                max_attempts: r.max_attempts,
            })
        })
        .collect())
}

pub struct AttemptOutcome {
    pub success: bool,
    pub exchange_id: ExchangeId,
}

/// Records one attempt's outcome: on success, marks the delivery
/// `succeeded` and resets the endpoint's failure streak; on failure,
/// reschedules (jittered sim-time retry) or marks `exhausted`, and
/// auto-disables the endpoint once its streak reaches 20 (spec §12.4).
/// Returns whether this call is what tipped the endpoint into disabled.
pub async fn record_attempt(
    pool: &SqlitePool,
    delivery_id: WebhookDeliveryId,
    endpoint_id: WebhookEndpointId,
    attempt_number: i64,
    max_attempts: i64,
    outcome: &AttemptOutcome,
    now: DateTime<Utc>,
) -> anyhow::Result<bool> {
    if outcome.success {
        sqlx::query(
            "UPDATE webhook_deliveries SET status = 'succeeded', attempt = ?1, last_exchange_id = ?2, delivered_at = ?3, next_attempt_at = NULL WHERE id = ?4",
        )
        .bind(attempt_number)
        .bind(outcome.exchange_id.to_string())
        .bind(now.to_rfc3339())
        .bind(delivery_id.to_string())
        .execute(pool)
        .await?;
        sqlx::query("UPDATE webhook_endpoints SET consecutive_failures = 0 WHERE id = ?1")
            .bind(endpoint_id.to_string())
            .execute(pool)
            .await?;
        return Ok(false);
    }

    let exhausted = attempt_number >= max_attempts;
    let next_attempt_at = if exhausted {
        None
    } else {
        crate::domain::webhook::next_retry_at(now, attempt_number)
    };
    let status = if exhausted { "exhausted" } else { "failed" };

    sqlx::query(
        "UPDATE webhook_deliveries SET status = ?1, attempt = ?2, last_exchange_id = ?3, next_attempt_at = ?4 WHERE id = ?5",
    )
    .bind(status)
    .bind(attempt_number)
    .bind(outcome.exchange_id.to_string())
    .bind(next_attempt_at.map(|t| t.to_rfc3339()))
    .bind(delivery_id.to_string())
    .execute(pool)
    .await?;

    let row: (i64,) = sqlx::query_as(
        "UPDATE webhook_endpoints SET consecutive_failures = consecutive_failures + 1 WHERE id = ?1 RETURNING consecutive_failures",
    )
    .bind(endpoint_id.to_string())
    .fetch_one(pool)
    .await?;

    if row.0 >= 20 {
        sqlx::query("UPDATE webhook_endpoints SET active = 0 WHERE id = ?1")
            .bind(endpoint_id.to_string())
            .execute(pool)
            .await?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::test_support::seed_merchant_and_provider;

    #[sqlx::test]
    async fn create_get_delete_round_trip(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();

        let id = create(
            &pool,
            &NewWebhookEndpoint {
                merchant_id,
                provider_slug,
                url: "https://example.test/hooks".into(),
                secret: "whsec_test_x".into(),
                enabled_events: "[]".into(),
                description: None,
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();

        assert!(get(&pool, merchant_id, id).await.unwrap().is_some());
        assert!(delete(&pool, merchant_id, id).await.unwrap());
        assert!(get(&pool, merchant_id, id).await.unwrap().is_none());
    }

    async fn new_endpoint(
        pool: &SqlitePool,
        merchant_id: MerchantId,
        provider_slug: &str,
        enabled_events: &str,
    ) -> WebhookEndpointId {
        create(
            pool,
            &NewWebhookEndpoint {
                merchant_id,
                provider_slug: provider_slug.into(),
                url: "https://example.test/hooks".into(),
                secret: "whsec_test_x".into(),
                enabled_events: enabled_events.into(),
                description: None,
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap()
    }

    /// Also inserts the matching `events` row `webhook_deliveries.event_id`
    /// foreign-keys into — in production this always exists already
    /// (`append_event` runs before `fan_out`); these tests exercise
    /// `fan_out` directly, so they must set that fixture up themselves.
    /// `record_attempt` writes `exchange_id` into
    /// `webhook_deliveries.last_exchange_id`, a foreign key into
    /// `http_exchanges` — a fresh, never-written `ExchangeId` fails that
    /// constraint, so these tests need a real row too, same as
    /// `dashboard::dispatcher` guarantees in production by writing the
    /// exchange (synchronously) before calling `record_attempt`.
    async fn insert_dummy_exchange(pool: &SqlitePool) -> ExchangeId {
        use crate::capture::recorder::{Channel, Exchange};
        let id = ExchangeId::new();
        let exchange = Exchange {
            id,
            trace_id: TraceId::new(),
            parent_id: None,
            direction: "outbound",
            channel: Channel::Webhook,
            started_at: Utc::now(),
            sim_at: Utc::now(),
            duration_ms: 5,
            method: "POST".into(),
            url: "https://example.test/hooks".into(),
            path: "https://example.test/hooks".into(),
            query: None,
            route_pattern: None,
            request_headers: vec![],
            request_body: None,
            status_code: Some(500),
            response_headers: vec![],
            response_body: None,
            outcome: "server_error",
            search_key: None,
            resources: vec![],
            delivery_id: None,
            event_id: None,
            attempt: None,
            signed_payload: None,
            signature: None,
        };
        crate::capture::recorder::write_exchange(pool, exchange)
            .await
            .unwrap();
        id
    }

    async fn fan_out_event(
        pool: &SqlitePool,
        merchant_id: MerchantId,
        provider_slug: &str,
        event_type: &str,
        now: DateTime<Utc>,
    ) -> FanOutEvent {
        let event_id = EventId::new();
        sqlx::query(
            "INSERT INTO events (id, merchant_id, provider_slug, type, resource_type, resource_id, data, created_at)
             VALUES (?1, ?2, ?3, ?4, 'shipment', 'shp_test', '{}', ?5)",
        )
        .bind(event_id.to_string())
        .bind(merchant_id.to_string())
        .bind(provider_slug)
        .bind(event_type)
        .bind(now.to_rfc3339())
        .execute(pool)
        .await
        .unwrap();

        FanOutEvent {
            merchant_id,
            provider_slug: provider_slug.into(),
            event_id,
            event_type: event_type.into(),
            trace_id: TraceId::new(),
            envelope: serde_json::json!({"id": "evt_x", "object": "event", "type": event_type}),
            now,
        }
    }

    #[sqlx::test]
    async fn fan_out_matches_wildcard_and_explicit_events(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        let wildcard = new_endpoint(&pool, merchant_id, &provider_slug, "[\"*\"]").await;
        let specific = new_endpoint(
            &pool,
            merchant_id,
            &provider_slug,
            "[\"shipment.delivered\"]",
        )
        .await;
        let unrelated =
            new_endpoint(&pool, merchant_id, &provider_slug, "[\"payment.captured\"]").await;

        let created = fan_out(
            &pool,
            &fan_out_event(
                &pool,
                merchant_id,
                &provider_slug,
                "shipment.delivered",
                now,
            )
            .await,
        )
        .await
        .unwrap();
        assert_eq!(created.len(), 2);

        let deliveries = list_deliveries_for_endpoint(&pool, wildcard).await.unwrap();
        assert_eq!(deliveries.len(), 1);
        let deliveries = list_deliveries_for_endpoint(&pool, specific).await.unwrap();
        assert_eq!(deliveries.len(), 1);
        let deliveries = list_deliveries_for_endpoint(&pool, unrelated)
            .await
            .unwrap();
        assert!(deliveries.is_empty());
    }

    #[sqlx::test]
    async fn fan_out_ignores_disabled_endpoints_and_other_providers(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        let disabled = new_endpoint(&pool, merchant_id, &provider_slug, "[\"*\"]").await;
        set_active(&pool, disabled, false).await.unwrap();

        let created = fan_out(
            &pool,
            &fan_out_event(
                &pool,
                merchant_id,
                &provider_slug,
                "shipment.delivered",
                now,
            )
            .await,
        )
        .await
        .unwrap();
        assert!(created.is_empty());

        let created = fan_out(
            &pool,
            &fan_out_event(&pool, merchant_id, "acmepay", "payment.captured", now).await,
        )
        .await
        .unwrap();
        assert!(created.is_empty());
    }

    #[sqlx::test]
    async fn due_returns_at_most_one_delivery_per_endpoint(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        let endpoint = new_endpoint(&pool, merchant_id, &provider_slug, "[\"*\"]").await;
        fan_out(
            &pool,
            &fan_out_event(
                &pool,
                merchant_id,
                &provider_slug,
                "shipment.picked_up",
                now,
            )
            .await,
        )
        .await
        .unwrap();
        fan_out(
            &pool,
            &fan_out_event(
                &pool,
                merchant_id,
                &provider_slug,
                "shipment.delivered",
                now,
            )
            .await,
        )
        .await
        .unwrap();

        let due_now = due(&pool, now, 10).await.unwrap();
        assert_eq!(due_now.len(), 1);
        assert_eq!(due_now[0].endpoint_id, endpoint);
    }

    #[sqlx::test]
    async fn record_attempt_reschedules_on_failure_and_exhausts_after_max_attempts(
        pool: SqlitePool,
    ) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        let endpoint = new_endpoint(&pool, merchant_id, &provider_slug, "[\"*\"]").await;
        fan_out(
            &pool,
            &fan_out_event(
                &pool,
                merchant_id,
                &provider_slug,
                "shipment.picked_up",
                now,
            )
            .await,
        )
        .await
        .unwrap();
        let delivery_id = due(&pool, now, 10).await.unwrap()[0].id;

        let outcome = AttemptOutcome {
            success: false,
            exchange_id: insert_dummy_exchange(&pool).await,
        };
        let disabled = record_attempt(&pool, delivery_id, endpoint, 1, 6, &outcome, now)
            .await
            .unwrap();
        assert!(!disabled);
        let detail = get_delivery(&pool, delivery_id).await.unwrap().unwrap();
        assert_eq!(detail.status, "failed");
        assert!(detail.next_attempt_at.is_some());

        let disabled = record_attempt(&pool, delivery_id, endpoint, 6, 6, &outcome, now)
            .await
            .unwrap();
        assert!(!disabled);
        let detail = get_delivery(&pool, delivery_id).await.unwrap().unwrap();
        assert_eq!(detail.status, "exhausted");
        assert!(detail.next_attempt_at.is_none());
    }

    #[sqlx::test]
    async fn record_attempt_auto_disables_after_twenty_consecutive_failures(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        let endpoint = new_endpoint(&pool, merchant_id, &provider_slug, "[\"*\"]").await;
        let outcome = AttemptOutcome {
            success: false,
            exchange_id: insert_dummy_exchange(&pool).await,
        };

        let mut disabled = false;
        for n in 1..=20 {
            fan_out(
                &pool,
                &fan_out_event(
                    &pool,
                    merchant_id,
                    &provider_slug,
                    "shipment.picked_up",
                    now,
                )
                .await,
            )
            .await
            .unwrap();
            let delivery_id = due(&pool, now, 10).await.unwrap()[0].id;
            disabled = record_attempt(&pool, delivery_id, endpoint, 1, 6, &outcome, now)
                .await
                .unwrap();
            if n < 20 {
                assert!(!disabled, "disabled too early at failure {n}");
            }
        }
        assert!(disabled, "endpoint should auto-disable at 20 failures");

        let row = get_unscoped(&pool, endpoint).await.unwrap().unwrap();
        assert!(!row.active);
    }

    #[sqlx::test]
    async fn record_attempt_success_resets_the_failure_streak(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        let endpoint = new_endpoint(&pool, merchant_id, &provider_slug, "[\"*\"]").await;
        fan_out(
            &pool,
            &fan_out_event(
                &pool,
                merchant_id,
                &provider_slug,
                "shipment.picked_up",
                now,
            )
            .await,
        )
        .await
        .unwrap();
        let delivery_id = due(&pool, now, 10).await.unwrap()[0].id;

        record_attempt(
            &pool,
            delivery_id,
            endpoint,
            1,
            6,
            &AttemptOutcome {
                success: false,
                exchange_id: insert_dummy_exchange(&pool).await,
            },
            now,
        )
        .await
        .unwrap();

        fan_out(
            &pool,
            &fan_out_event(
                &pool,
                merchant_id,
                &provider_slug,
                "shipment.delivered",
                now,
            )
            .await,
        )
        .await
        .unwrap();
        let deliveries = list_deliveries_for_endpoint(&pool, endpoint).await.unwrap();
        let second = deliveries
            .iter()
            .find(|d| d.event_type == "shipment.delivered")
            .unwrap();

        record_attempt(
            &pool,
            second.id,
            endpoint,
            1,
            6,
            &AttemptOutcome {
                success: true,
                exchange_id: insert_dummy_exchange(&pool).await,
            },
            now,
        )
        .await
        .unwrap();

        let row = get_unscoped(&pool, endpoint).await.unwrap().unwrap();
        assert_eq!(row.consecutive_failures, 0);
    }
}
