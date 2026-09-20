//! Payment persistence (spec §7.3): `payments`, `checkout_sessions`,
//! `payment_events`, `refunds`, `card_tokens`. Every status write also
//! appends the matching `payment_events`/`events` rows in the same
//! transaction, mirroring `db::repo::shipments`' rule.

use chrono::{DateTime, Utc};
use sqlx::{Sqlite, SqlitePool, Transaction};
use ulid::Ulid;

use crate::domain::event::EventType;
use crate::domain::ids::{CheckoutSessionId, EventId, MerchantId, PaymentId, RefundId};
use crate::domain::payment::PaymentStatus;

/// Best-effort fan-out to any matching webhook endpoints (specs/005-Webhooks.md
/// item 4). Not part of the resource's own transaction — a webhook is a
/// side effect of the event existing, not a precondition for the resource
/// write to succeed, mirroring how `dashboard::activity::publish` below is
/// also called post-commit rather than inside the transaction.
async fn fan_out_payment_event(
    pool: &SqlitePool,
    merchant_id: MerchantId,
    provider_slug: &str,
    event_id: EventId,
    event_type: &str,
    occurred_at: DateTime<Utc>,
    resource: serde_json::Value,
) {
    let envelope = crate::domain::webhook::build_envelope(
        &event_id.to_string(),
        event_type,
        occurred_at,
        resource,
    );
    let trace_id = crate::capture::trace::current_trace_id().unwrap_or_default();
    tracing::debug!(%merchant_id, provider_slug, event_type, %event_id, "payment flow: fanning out event to webhook endpoints");
    match crate::db::repo::webhooks::fan_out(
        pool,
        &crate::db::repo::webhooks::FanOutEvent {
            merchant_id,
            provider_slug: provider_slug.to_string(),
            event_id,
            event_type: event_type.to_string(),
            trace_id,
            envelope,
            now: occurred_at,
        },
    )
    .await
    {
        Ok(created) if created.is_empty() => {
            tracing::info!(%merchant_id, provider_slug, event_type, %event_id, "payment flow: no active webhook endpoint subscribed to this event, nothing queued");
        }
        Ok(created) => {
            tracing::info!(%merchant_id, provider_slug, event_type, %event_id, deliveries = created.len(), "payment flow: queued webhook deliveries");
        }
        Err(error) => {
            tracing::error!(?error, %event_id, "payment flow: webhook fan-out failed");
        }
    }
}

pub struct NewPayment {
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub amount_cents: i64,
    pub currency: String,
    pub capture_mode: String,
    pub reference: Option<String>,
    pub method_kind: String,
    pub method_detail: Option<String>,
    pub installments: i32,
    pub scenario: String,
    pub risk_score: Option<i64>,
    pub risk_decision: Option<String>,
    pub three_ds: Option<String>,
    pub metadata: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct PaymentRow {
    pub id: PaymentId,
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub amount_cents: i64,
    pub currency: String,
    pub amount_refunded_cents: i64,
    pub status: PaymentStatus,
    pub status_reason: Option<String>,
    pub capture_mode: String,
    pub reference: Option<String>,
    pub method_kind: String,
    pub method_detail: Option<String>,
    pub installments: i32,
    pub auth_code: Option<String>,
    pub response_code: Option<i32>,
    pub risk_score: Option<i32>,
    pub risk_decision: Option<String>,
    pub three_ds: Option<String>,
    pub scenario: Option<String>,
    pub metadata: Option<String>,
    pub created_at: DateTime<Utc>,
    pub captured_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct PaymentSqlRow {
    id: String,
    merchant_id: String,
    provider_slug: String,
    amount_cents: i64,
    currency: String,
    amount_refunded_cents: i64,
    status: PaymentStatus,
    status_reason: Option<String>,
    capture_mode: String,
    reference: Option<String>,
    method_kind: String,
    method_detail: Option<String>,
    installments: i64,
    auth_code: Option<String>,
    response_code: Option<i64>,
    risk_score: Option<i64>,
    risk_decision: Option<String>,
    three_ds: Option<String>,
    scenario: Option<String>,
    metadata: Option<String>,
    created_at: String,
    captured_at: Option<String>,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .expect("stored timestamps are valid rfc3339")
        .with_timezone(&Utc)
}

impl PaymentSqlRow {
    fn into_row(self) -> PaymentRow {
        PaymentRow {
            id: self.id.parse().expect("stored payment id is valid"),
            merchant_id: self
                .merchant_id
                .parse()
                .expect("stored merchant id is valid"),
            provider_slug: self.provider_slug,
            amount_cents: self.amount_cents,
            currency: self.currency,
            amount_refunded_cents: self.amount_refunded_cents,
            status: self.status,
            status_reason: self.status_reason,
            capture_mode: self.capture_mode,
            reference: self.reference,
            method_kind: self.method_kind,
            method_detail: self.method_detail,
            installments: self.installments as i32,
            auth_code: self.auth_code,
            response_code: self.response_code.map(|v| v as i32),
            risk_score: self.risk_score.map(|v| v as i32),
            risk_decision: self.risk_decision,
            three_ds: self.three_ds,
            scenario: self.scenario,
            metadata: self.metadata,
            created_at: parse_dt(&self.created_at),
            captured_at: self.captured_at.as_deref().map(parse_dt),
        }
    }
}

const PAYMENT_COLUMNS: &str = "id, merchant_id, provider_slug, amount_cents, currency, amount_refunded_cents,
     status, status_reason, capture_mode, external_ref AS reference, method_kind, method_detail, installments,
     auth_code, response_code, risk_score, risk_decision, three_ds, scenario, metadata, created_at, captured_at";

/// Inserts the payment at `Created` and appends its first `payment_events`
/// row. Callers then drive it forward with `advance` for every
/// zero-delay scenario step (spec §9.1), all inside one logical creation.
pub async fn create(pool: &SqlitePool, new: &NewPayment) -> anyhow::Result<PaymentId> {
    let id = PaymentId::new();
    let now = new.created_at;

    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO payments (
            id, merchant_id, provider_slug, order_id, session_id, external_ref,
            amount_cents, currency, amount_refunded_cents, status, status_reason,
            provider_status, provider_status_detail, capture_mode, method_kind, method_detail,
            installments, auth_code, response_code, risk_score, risk_decision, three_ds,
            redirect_url, return_url, cancel_url, scenario, metadata, next_transition_at,
            authorized_at, captured_at, expires_at, created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, NULL, NULL, ?4,
            ?5, ?6, 0, ?7, NULL,
            NULL, NULL, ?8, ?9, ?10,
            ?11, NULL, NULL, ?12, ?13, ?14,
            NULL, NULL, NULL, ?15, ?16, NULL,
            NULL, NULL, NULL, ?17, ?17
        )",
    )
    .bind(id.to_string())
    .bind(new.merchant_id.to_string())
    .bind(&new.provider_slug)
    .bind(&new.reference)
    .bind(new.amount_cents)
    .bind(&new.currency)
    .bind(PaymentStatus::Created)
    .bind(&new.capture_mode)
    .bind(&new.method_kind)
    .bind(&new.method_detail)
    .bind(new.installments as i64)
    .bind(new.risk_score)
    .bind(&new.risk_decision)
    .bind(&new.three_ds)
    .bind(&new.scenario)
    .bind(&new.metadata)
    .bind(now.to_rfc3339())
    .execute(&mut *tx)
    .await?;

    // `Created` has no `EventType` variant — it's the row's initial state,
    // never the result of an `apply()` transition — so this first row uses
    // a literal label rather than the shared enum the real transitions use.
    let snapshot = serde_json::json!({
        "id": id.to_string(),
        "object": "payment",
        "status": PaymentStatus::Created,
        "amount": new.amount_cents,
        "amount_refunded": 0,
        "currency": new.currency,
        "reference": new.reference,
        "capture_mode": new.capture_mode,
        "livemode": false,
        "created_at": now.to_rfc3339(),
        "captured_at": Option::<String>::None,
        "metadata": new.metadata.as_deref().and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok()),
    });
    let event_id = append_event(
        &mut tx,
        id,
        new.merchant_id,
        &new.provider_slug,
        0,
        PaymentStatus::Created,
        "payment.created",
        now,
        &serde_json::to_string(&snapshot)?,
    )
    .await?;

    tx.commit().await?;

    fan_out_payment_event(
        pool,
        new.merchant_id,
        &new.provider_slug,
        event_id,
        "payment.created",
        now,
        snapshot,
    )
    .await;

    Ok(id)
}

/// Applies one already-validated transition: updates `payments` and
/// appends the matching event rows, mirroring `shipments::advance`.
#[allow(clippy::too_many_arguments)]
pub async fn advance(
    pool: &SqlitePool,
    id: PaymentId,
    merchant_id: MerchantId,
    provider_slug: &str,
    to: PaymentStatus,
    event: EventType,
    seq: i64,
    occurred_at: DateTime<Utc>,
    next_transition_at: Option<DateTime<Utc>>,
    status_reason: Option<&str>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;

    let captured_at = matches!(to, PaymentStatus::Captured).then(|| occurred_at.to_rfc3339());
    let authorized_at = matches!(to, PaymentStatus::Authorized).then(|| occurred_at.to_rfc3339());

    sqlx::query(
        "UPDATE payments SET status = ?1, status_reason = ?2, next_transition_at = ?3, updated_at = ?4,
            captured_at = COALESCE(?5, captured_at), authorized_at = COALESCE(?6, authorized_at)
         WHERE id = ?7",
    )
    .bind(to)
    .bind(status_reason)
    .bind(next_transition_at.map(|t| t.to_rfc3339()))
    .bind(occurred_at.to_rfc3339())
    .bind(captured_at)
    .bind(authorized_at)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;

    // The dialect-shaped snapshot `events.data` is documented to carry
    // (spec §7.5) — read back post-update rather than threaded through
    // every caller of `advance`, so this fix (specs/005-Webhooks.md item 3)
    // touches this one function instead of two dozen call sites.
    let snapshot_row: (i64, i64, String, Option<String>, String, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT amount_cents, amount_refunded_cents, currency, external_ref, capture_mode, metadata, captured_at
             FROM payments WHERE id = ?1",
        )
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await?;
    let (
        amount_cents,
        amount_refunded_cents,
        currency,
        reference,
        capture_mode,
        metadata,
        captured_at,
    ) = snapshot_row;
    let snapshot = serde_json::json!({
        "id": id.to_string(),
        "object": "payment",
        "status": to,
        "amount": amount_cents,
        "amount_refunded": amount_refunded_cents,
        "currency": currency,
        "reference": reference,
        "capture_mode": capture_mode,
        "livemode": false,
        "captured_at": captured_at,
        "metadata": metadata.as_deref().and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok()),
    });

    let event_id = append_event(
        &mut tx,
        id,
        merchant_id,
        provider_slug,
        seq,
        to,
        event.as_str(),
        occurred_at,
        &serde_json::to_string(&snapshot)?,
    )
    .await?;

    tx.commit().await?;

    crate::dashboard::activity::publish(crate::dashboard::activity::ActivityEvent::resource(
        "payment",
        id.to_string(),
        format!("{id} \u{2192} {event}"),
        occurred_at,
    ));

    fan_out_payment_event(
        pool,
        merchant_id,
        provider_slug,
        event_id,
        event.as_str(),
        occurred_at,
        snapshot,
    )
    .await;

    Ok(())
}

/// Sets `next_transition_at` without otherwise changing the row — used
/// right after `create` to schedule a scenario's first delayed hop.
pub async fn schedule_next(
    pool: &SqlitePool,
    id: PaymentId,
    next_transition_at: Option<DateTime<Utc>>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE payments SET next_transition_at = ?1 WHERE id = ?2")
        .bind(next_transition_at.map(|t| t.to_rfc3339()))
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub struct DuePayment {
    pub id: PaymentId,
    pub status: PaymentStatus,
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub scenario: Option<String>,
}

#[derive(sqlx::FromRow)]
struct DueRow {
    id: String,
    status: PaymentStatus,
    merchant_id: String,
    provider_slug: String,
    scenario: Option<String>,
}

pub async fn due(pool: &SqlitePool, now: DateTime<Utc>) -> anyhow::Result<Vec<DuePayment>> {
    let rows: Vec<DueRow> = sqlx::query_as(
        "SELECT id, status, merchant_id, provider_slug, scenario
         FROM payments
         WHERE next_transition_at IS NOT NULL AND next_transition_at <= ?1
         ORDER BY next_transition_at ASC",
    )
    .bind(now.to_rfc3339())
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Ok(id) = row.id.parse::<PaymentId>() else {
            continue;
        };
        let Ok(merchant_id) = row.merchant_id.parse::<MerchantId>() else {
            continue;
        };
        out.push(DuePayment {
            id,
            status: row.status,
            merchant_id,
            provider_slug: row.provider_slug,
            scenario: row.scenario,
        });
    }
    Ok(out)
}

pub struct PaymentEventRow {
    pub kind: String,
    pub source: String,
    pub occurred_at: DateTime<Utc>,
}

/// The payment's timeline (spec §13.6's payment detail: "a vertical
/// timeline built from `payment_events` showing sim timestamps and the
/// source of each transition").
pub async fn list_events(
    pool: &SqlitePool,
    payment_id: PaymentId,
) -> anyhow::Result<Vec<PaymentEventRow>> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT type, source, occurred_at FROM payment_events WHERE payment_id = ?1 ORDER BY seq ASC",
    )
    .bind(payment_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(kind, source, occurred_at)| PaymentEventRow {
            kind,
            source,
            occurred_at: parse_dt(&occurred_at),
        })
        .collect())
}

pub async fn event_count(pool: &SqlitePool, payment_id: PaymentId) -> anyhow::Result<i64> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM payment_events WHERE payment_id = ?1")
            .bind(payment_id.to_string())
            .fetch_one(pool)
            .await?;
    Ok(count)
}

pub async fn get(pool: &SqlitePool, id: PaymentId) -> anyhow::Result<Option<PaymentRow>> {
    let row: Option<PaymentSqlRow> = sqlx::query_as(&format!(
        "SELECT {PAYMENT_COLUMNS} FROM payments WHERE id = ?1"
    ))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(PaymentSqlRow::into_row))
}

/// `merchant_id`/`provider_slug` are `Some` for the merchant-scoped
/// provider APIs (M2's own callers) and `None` for the dashboard's
/// unscoped, cross-merchant view (specs/004-Dashboard.md — the dashboard
/// has no auth, spec §18, so there is no merchant to scope to). `search`
/// matches `reference` as a substring, for the dashboard's filter row.
pub struct ListFilter {
    pub merchant_id: Option<MerchantId>,
    pub provider_slug: Option<String>,
    pub status: Option<PaymentStatus>,
    pub created_after: Option<DateTime<Utc>>,
    pub search: Option<String>,
    pub limit: i64,
    pub starting_after: Option<PaymentId>,
}

/// Cursor-paginated list, newest first (spec §10.1's Stripe-style cursor
/// idiom): `starting_after` is the last id of the previous page.
pub async fn list(pool: &SqlitePool, filter: &ListFilter) -> anyhow::Result<Vec<PaymentRow>> {
    let mut sql = format!("SELECT {PAYMENT_COLUMNS} FROM payments WHERE 1=1");
    let mut n = 0;
    if filter.merchant_id.is_some() {
        n += 1;
        sql.push_str(&format!(" AND merchant_id = ?{n}"));
    }
    if filter.provider_slug.is_some() {
        n += 1;
        sql.push_str(&format!(" AND provider_slug = ?{n}"));
    }
    if filter.status.is_some() {
        n += 1;
        sql.push_str(&format!(" AND status = ?{n}"));
    }
    if filter.created_after.is_some() {
        n += 1;
        sql.push_str(&format!(" AND created_at > ?{n}"));
    }
    if filter.search.is_some() {
        n += 1;
        sql.push_str(&format!(" AND external_ref LIKE ?{n}"));
    }
    if filter.starting_after.is_some() {
        n += 1;
        sql.push_str(&format!(" AND id < ?{n}"));
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");
    n += 1;
    sql.push_str(&n.to_string());

    let mut query = sqlx::query_as::<_, PaymentSqlRow>(&sql);
    if let Some(merchant_id) = filter.merchant_id {
        query = query.bind(merchant_id.to_string());
    }
    if let Some(provider_slug) = &filter.provider_slug {
        query = query.bind(provider_slug);
    }
    if let Some(status) = filter.status {
        query = query.bind(status);
    }
    if let Some(created_after) = filter.created_after {
        query = query.bind(created_after.to_rfc3339());
    }
    if let Some(search) = &filter.search {
        query = query.bind(format!("%{search}%"));
    }
    if let Some(starting_after) = filter.starting_after {
        query = query.bind(starting_after.to_string());
    }
    query = query.bind(filter.limit);

    let rows = query.fetch_all(pool).await?;
    Ok(rows.into_iter().map(PaymentSqlRow::into_row).collect())
}

pub struct NewRefund {
    pub payment_id: PaymentId,
    pub amount_cents: i64,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct RefundRow {
    pub id: RefundId,
    pub payment_id: PaymentId,
    pub amount_cents: i64,
    pub reason: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct RefundSqlRow {
    id: String,
    payment_id: String,
    amount_cents: i64,
    reason: Option<String>,
    status: String,
    created_at: String,
}

/// Inserts the refund row, bumps `payments.amount_refunded_cents`, and
/// writes the matching `payment_events`/`events` rows — the *status*
/// transition (`captured` -> `partially_refunded`/`refunded`) is applied by
/// the caller via `advance` first; this only records the refund itself.
pub async fn create_refund(
    pool: &SqlitePool,
    new: &NewRefund,
    merchant_id: MerchantId,
    provider_slug: &str,
) -> anyhow::Result<RefundId> {
    let id = RefundId::new();
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO refunds (id, payment_id, amount_cents, reason, status, provider_ref, nullified, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'succeeded', NULL, 0, ?5, ?5)",
    )
    .bind(id.to_string())
    .bind(new.payment_id.to_string())
    .bind(new.amount_cents)
    .bind(&new.reason)
    .bind(new.created_at.to_rfc3339())
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE payments SET amount_refunded_cents = amount_refunded_cents + ?1, updated_at = ?2 WHERE id = ?3")
        .bind(new.amount_cents)
        .bind(new.created_at.to_rfc3339())
        .bind(new.payment_id.to_string())
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO events (id, merchant_id, provider_slug, type, resource_type, resource_id, data, created_at)
         VALUES (?1, ?2, ?3, ?4, 'refund', ?5, ?6, ?7)",
    )
    .bind(EventId::new().to_string())
    .bind(merchant_id.to_string())
    .bind(provider_slug)
    .bind("refund.created")
    .bind(id.to_string())
    .bind(format!(r#"{{"payment_id":"{}","amount_cents":{}}}"#, new.payment_id, new.amount_cents))
    .bind(new.created_at.to_rfc3339())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(id)
}

pub async fn list_refunds(
    pool: &SqlitePool,
    payment_id: PaymentId,
) -> anyhow::Result<Vec<RefundRow>> {
    let rows: Vec<RefundSqlRow> = sqlx::query_as(
        "SELECT id, payment_id, amount_cents, reason, status, created_at FROM refunds WHERE payment_id = ?1 ORDER BY created_at",
    )
    .bind(payment_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RefundRow {
            id: r.id.parse().expect("stored refund id is valid"),
            payment_id: r.payment_id.parse().expect("stored payment id is valid"),
            amount_cents: r.amount_cents,
            reason: r.reason,
            status: r.status,
            created_at: parse_dt(&r.created_at),
        })
        .collect())
}

pub struct NewCheckoutSession {
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub amount_cents: i64,
    pub currency: String,
    pub line_items: Option<String>,
    pub customer_email: Option<String>,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub hosted_url: String,
}

pub struct CheckoutSessionRow {
    pub id: CheckoutSessionId,
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub payment_id: Option<PaymentId>,
    pub status: String,
    pub payment_status: String,
    pub amount_cents: i64,
    pub currency: String,
    pub hosted_url: String,
    /// Opaque, dialect-owned JSON (spec §10.1 doc: "opaque line-item data,
    /// echoed back unchanged") — Trancorp Webpay repurposes this to carry
    /// `{buy_order, session_id}` instead of Acme Pay's actual line items,
    /// since neither dialect's use requires a dedicated column.
    pub line_items: Option<String>,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct CheckoutSqlRow {
    id: String,
    merchant_id: String,
    provider_slug: String,
    payment_id: Option<String>,
    status: String,
    payment_status: String,
    amount_cents: i64,
    currency: String,
    hosted_url: String,
    line_items: Option<String>,
    success_url: Option<String>,
    cancel_url: Option<String>,
    expires_at: String,
    created_at: String,
}

impl CheckoutSqlRow {
    fn into_row(self) -> CheckoutSessionRow {
        CheckoutSessionRow {
            id: self
                .id
                .parse()
                .expect("stored checkout session id is valid"),
            merchant_id: self
                .merchant_id
                .parse()
                .expect("stored merchant id is valid"),
            provider_slug: self.provider_slug,
            payment_id: self.payment_id.and_then(|p| p.parse().ok()),
            status: self.status,
            payment_status: self.payment_status,
            amount_cents: self.amount_cents,
            currency: self.currency,
            hosted_url: self.hosted_url,
            line_items: self.line_items,
            success_url: self.success_url,
            cancel_url: self.cancel_url,
            expires_at: parse_dt(&self.expires_at),
            created_at: parse_dt(&self.created_at),
        }
    }
}

/// `id` is generated by the caller (not internally) so the handler can
/// build `hosted_url` from it before the row exists.
pub async fn create_checkout_session(
    pool: &SqlitePool,
    id: CheckoutSessionId,
    new: &NewCheckoutSession,
) -> anyhow::Result<()> {
    let token = Ulid::new().to_string();

    sqlx::query(
        "INSERT INTO checkout_sessions (
            id, merchant_id, provider_slug, payment_id, token, status, payment_status,
            amount_cents, currency, line_items, customer_email, success_url, cancel_url,
            hosted_url, expires_at, created_at, updated_at
        ) VALUES (?1, ?2, ?3, NULL, ?4, 'open', 'unpaid', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
    )
    .bind(id.to_string())
    .bind(new.merchant_id.to_string())
    .bind(&new.provider_slug)
    .bind(&token)
    .bind(new.amount_cents)
    .bind(&new.currency)
    .bind(&new.line_items)
    .bind(&new.customer_email)
    .bind(&new.success_url)
    .bind(&new.cancel_url)
    .bind(&new.hosted_url)
    .bind(new.expires_at.to_rfc3339())
    .bind(new.created_at.to_rfc3339())
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_checkout_session(
    pool: &SqlitePool,
    id: CheckoutSessionId,
) -> anyhow::Result<Option<CheckoutSessionRow>> {
    let row: Option<CheckoutSqlRow> = sqlx::query_as(
        "SELECT id, merchant_id, provider_slug, payment_id, status, payment_status,
                amount_cents, currency, hosted_url, line_items, success_url, cancel_url,
                expires_at, created_at
         FROM checkout_sessions WHERE id = ?1",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(CheckoutSqlRow::into_row))
}

pub async fn expire_checkout_session(
    pool: &SqlitePool,
    id: CheckoutSessionId,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE checkout_sessions SET status = 'expired', updated_at = ?1 WHERE id = ?2")
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

/// Stores an arbitrary string in `payment_status` without otherwise
/// changing the row. Every other caller of this column uses it for its
/// documented `unpaid`/`paid` values; Trancorp Webpay's create→commit flow
/// (specs/006-Dialects.md item 2) repurposes it to hold the *pending
/// scenario name* between create and commit — `unpaid`/`paid` themselves
/// are never valid `domain::scenario::PaymentScenario` strings, so the two
/// uses can never collide, and the hosted checkout page's forced-outcome
/// buttons are what call this, overwriting whatever `create` resolved
/// from the amount-suffix magic values.
pub async fn set_checkout_session_payment_status(
    pool: &SqlitePool,
    id: CheckoutSessionId,
    payment_status: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE checkout_sessions SET payment_status = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(payment_status)
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

/// Finalizes a checkout session once its one-time commit has happened
/// (Trancorp Webpay, specs/006-Dialects.md item 2's create→commit flow):
/// attaches the resulting `payment_id`, sets the real `paid`/`unpaid`
/// `payment_status`, and moves `status` to `closed` — a second commit
/// attempt then finds a non-`open` session and 422s, matching §10.2's
/// "Transaction already locked".
pub async fn close_checkout_session(
    pool: &SqlitePool,
    id: CheckoutSessionId,
    payment_id: PaymentId,
    payment_status: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE checkout_sessions SET status = 'closed', payment_id = ?1, payment_status = ?2, updated_at = ?3 WHERE id = ?4",
    )
    .bind(payment_id.to_string())
    .bind(payment_status)
    .bind(now.to_rfc3339())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

/// `brand`/`last4` only — the full PAN is never persisted (spec §18).
#[allow(clippy::too_many_arguments)]
pub async fn store_card_token(
    pool: &SqlitePool,
    merchant_id: MerchantId,
    brand: &str,
    last4: &str,
    exp_month: i32,
    exp_year: i32,
    holder: Option<&str>,
    scenario: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<String> {
    let token = format!("tok_{}", Ulid::new());
    sqlx::query(
        "INSERT INTO card_tokens (token, merchant_id, brand, last4, exp_month, exp_year, holder, scenario, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(&token)
    .bind(merchant_id.to_string())
    .bind(brand)
    .bind(last4)
    .bind(exp_month as i64)
    .bind(exp_year as i64)
    .bind(holder)
    .bind(scenario)
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;
    Ok(token)
}

#[allow(clippy::too_many_arguments)]
async fn append_event(
    tx: &mut Transaction<'_, Sqlite>,
    payment_id: PaymentId,
    merchant_id: MerchantId,
    provider_slug: &str,
    seq: i64,
    status: PaymentStatus,
    event: &str,
    occurred_at: DateTime<Utc>,
    data: &str,
) -> anyhow::Result<EventId> {
    let occurred_at_text = occurred_at.to_rfc3339();

    sqlx::query(
        "INSERT INTO payment_events (id, payment_id, seq, type, from_status, to_status, source, data, occurred_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'simulation', NULL, ?6)",
    )
    .bind(Ulid::new().to_string())
    .bind(payment_id.to_string())
    .bind(seq)
    .bind(event)
    .bind(status)
    .bind(&occurred_at_text)
    .execute(&mut **tx)
    .await?;

    let event_id = EventId::new();
    sqlx::query(
        "INSERT INTO events (id, merchant_id, provider_slug, type, resource_type, resource_id, data, created_at)
         VALUES (?1, ?2, ?3, ?4, 'payment', ?5, ?6, ?7)",
    )
    .bind(event_id.to_string())
    .bind(merchant_id.to_string())
    .bind(provider_slug)
    .bind(event)
    .bind(payment_id.to_string())
    .bind(data)
    .bind(&occurred_at_text)
    .execute(&mut **tx)
    .await?;

    Ok(event_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::test_support::seed_merchant_and_provider;

    fn new_payment(
        merchant_id: MerchantId,
        provider_slug: String,
        now: DateTime<Utc>,
    ) -> NewPayment {
        NewPayment {
            merchant_id,
            provider_slug,
            amount_cents: 10_000,
            currency: "USD".into(),
            capture_mode: "automatic".into(),
            reference: Some("ORD-1".into()),
            method_kind: "card".into(),
            method_detail: None,
            installments: 1,
            scenario: "approve".into(),
            risk_score: None,
            risk_decision: None,
            three_ds: None,
            metadata: None,
            created_at: now,
        }
    }

    #[sqlx::test]
    async fn create_inserts_payment_and_first_event(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        let id = create(&pool, &new_payment(merchant_id, provider_slug, now))
            .await
            .unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.status, PaymentStatus::Created);
        assert_eq!(event_count(&pool, id).await.unwrap(), 1);
    }

    #[sqlx::test]
    async fn advance_updates_status_and_appends_event(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        let id = create(&pool, &new_payment(merchant_id, provider_slug.clone(), now))
            .await
            .unwrap();
        advance(
            &pool,
            id,
            merchant_id,
            &provider_slug,
            PaymentStatus::Pending,
            EventType::PaymentPending,
            1,
            now,
            None,
            None,
        )
        .await
        .unwrap();
        advance(
            &pool,
            id,
            merchant_id,
            &provider_slug,
            PaymentStatus::Authorized,
            EventType::PaymentAuthorized,
            2,
            now,
            None,
            None,
        )
        .await
        .unwrap();
        advance(
            &pool,
            id,
            merchant_id,
            &provider_slug,
            PaymentStatus::Captured,
            EventType::PaymentCaptured,
            3,
            now,
            None,
            None,
        )
        .await
        .unwrap();

        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.status, PaymentStatus::Captured);
        assert!(row.captured_at.is_some());
        assert_eq!(event_count(&pool, id).await.unwrap(), 4);
    }

    #[sqlx::test]
    async fn list_filters_by_status_and_paginates(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        for _ in 0..3 {
            create(&pool, &new_payment(merchant_id, provider_slug.clone(), now))
                .await
                .unwrap();
        }

        let all = list(
            &pool,
            &ListFilter {
                merchant_id: Some(merchant_id),
                provider_slug: Some(provider_slug.clone()),
                status: None,
                created_after: None,
                search: None,
                limit: 2,
                starting_after: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(all.len(), 2);

        let filtered = list(
            &pool,
            &ListFilter {
                merchant_id: Some(merchant_id),
                provider_slug: Some(provider_slug),
                status: Some(PaymentStatus::Created),
                created_after: None,
                search: None,
                limit: 10,
                starting_after: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(filtered.len(), 3);
    }

    #[sqlx::test]
    async fn list_unscoped_returns_rows_across_merchants(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        create(&pool, &new_payment(merchant_id, provider_slug, now))
            .await
            .unwrap();

        let all = list(
            &pool,
            &ListFilter {
                merchant_id: None,
                provider_slug: None,
                status: None,
                created_after: None,
                search: None,
                limit: 10,
                starting_after: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(all.len(), 1);
    }

    #[sqlx::test]
    async fn refund_bumps_amount_refunded(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();
        let id = create(&pool, &new_payment(merchant_id, provider_slug.clone(), now))
            .await
            .unwrap();

        create_refund(
            &pool,
            &NewRefund {
                payment_id: id,
                amount_cents: 50_00,
                reason: None,
                created_at: now,
            },
            merchant_id,
            &provider_slug,
        )
        .await
        .unwrap();

        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.amount_refunded_cents, 50_00);
        assert_eq!(list_refunds(&pool, id).await.unwrap().len(), 1);
    }
}
