//! Shipment persistence (spec §7.4). Every write here also appends the
//! matching `shipment_events`/`events` rows in the same transaction, so the
//! event log can never drift from `shipments.status`.

use chrono::{DateTime, Utc};
use sqlx::{Sqlite, SqlitePool, Transaction};
use ulid::Ulid;

use crate::domain::event::EventType;
use crate::domain::ids::{EventId, MerchantId, PickupId, RateOptionId, RateQuoteId, ShipmentId};
use crate::domain::shipment::ShipmentStatus;

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .expect("stored timestamps are valid rfc3339")
        .with_timezone(&Utc)
}

pub struct NewShipment {
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub carrier_code: String,
    pub service_code: String,
    pub origin: String,
    pub destination: String,
    pub tracking_number: String,
    pub price_cents: i64,
    pub currency: String,
    pub created_at: DateTime<Utc>,
    pub eta_at: DateTime<Utc>,
}

pub struct DueShipment {
    pub id: ShipmentId,
    pub status: ShipmentStatus,
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub created_at: DateTime<Utc>,
    pub eta_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct DueRow {
    id: String,
    status: ShipmentStatus,
    merchant_id: String,
    provider_slug: String,
    created_at: String,
    eta_at: Option<String>,
}

/// Inserts the shipment at `Created` (schedule hop 0) and its matching
/// `shipment_events`/`events` rows, due immediately so the ticker picks it
/// up and cascades through `LabelGenerated` (hop 1, also at t+0) on its
/// first tick.
pub async fn create(pool: &SqlitePool, new: &NewShipment) -> anyhow::Result<ShipmentId> {
    let id = ShipmentId::new();
    let now = new.created_at;

    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO shipments (
            id, merchant_id, provider_slug, order_id, rate_option_id,
            tracking_number, external_ref, carrier_code, service_code,
            origin, destination, price_cents, currency, declared_value_cents,
            status, status_reason, provider_status_code, label_url, label_format,
            eta_at, promised_window, delivery_attempts, proof_of_delivery,
            return_tracking_number, scenario, next_transition_at,
            created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, NULL, NULL,
            ?4, NULL, ?5, ?6,
            ?7, ?8, ?9, ?10, 0,
            ?11, NULL, NULL, NULL, NULL,
            ?12, NULL, 0, NULL,
            NULL, NULL, ?13,
            ?13, ?13
        )",
    )
    .bind(id.to_string())
    .bind(new.merchant_id.to_string())
    .bind(&new.provider_slug)
    .bind(&new.tracking_number)
    .bind(&new.carrier_code)
    .bind(&new.service_code)
    .bind(&new.origin)
    .bind(&new.destination)
    .bind(new.price_cents)
    .bind(&new.currency)
    .bind(ShipmentStatus::Created)
    .bind(new.eta_at.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(&mut *tx)
    .await?;

    append_event(
        &mut tx,
        id,
        new.merchant_id,
        &new.provider_slug,
        0,
        ShipmentStatus::Created,
        EventType::ShipmentCreated,
        now,
    )
    .await?;

    tx.commit().await?;
    Ok(id)
}

/// Rows whose `next_transition_at` is due, oldest first.
pub async fn due(pool: &SqlitePool, now: DateTime<Utc>) -> anyhow::Result<Vec<DueShipment>> {
    let rows: Vec<DueRow> = sqlx::query_as(
        "SELECT id, status, merchant_id, provider_slug, created_at, eta_at
         FROM shipments
         WHERE next_transition_at IS NOT NULL AND next_transition_at <= ?1
         ORDER BY next_transition_at ASC",
    )
    .bind(now.to_rfc3339())
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Ok(id) = row.id.parse::<ShipmentId>() else {
            tracing::warn!(id = %row.id, "skipping shipment with unparseable id");
            continue;
        };
        let Ok(merchant_id) = row.merchant_id.parse::<MerchantId>() else {
            tracing::warn!(id = %row.id, "skipping shipment with unparseable merchant_id");
            continue;
        };
        let Ok(created_at) = DateTime::parse_from_rfc3339(&row.created_at) else {
            tracing::warn!(id = %row.id, "skipping shipment with unparseable created_at");
            continue;
        };
        let eta_at = row
            .eta_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        out.push(DueShipment {
            id,
            status: row.status,
            merchant_id,
            provider_slug: row.provider_slug,
            created_at: created_at.with_timezone(&Utc),
            eta_at,
        });
    }

    Ok(out)
}

/// Number of `shipment_events` rows recorded so far — doubles as the next
/// `seq` and as the index into a regenerated happy-path schedule (spec
/// §8.2's schedule is deterministic given `(eta, seed)`, so it never needs
/// to be persisted separately).
pub struct ShipmentEventRow {
    pub status: ShipmentStatus,
    pub description: String,
    pub occurred_at: DateTime<Utc>,
}

pub async fn list_events(
    pool: &SqlitePool,
    shipment_id: ShipmentId,
) -> anyhow::Result<Vec<ShipmentEventRow>> {
    let rows: Vec<(ShipmentStatus, String, String)> = sqlx::query_as(
        "SELECT status, description, occurred_at FROM shipment_events WHERE shipment_id = ?1 ORDER BY seq ASC",
    )
    .bind(shipment_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(status, description, occurred_at)| ShipmentEventRow {
            status,
            description,
            occurred_at: parse_dt(&occurred_at),
        })
        .collect())
}

pub async fn event_count(pool: &SqlitePool, shipment_id: ShipmentId) -> anyhow::Result<i64> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM shipment_events WHERE shipment_id = ?1")
            .bind(shipment_id.to_string())
            .fetch_one(pool)
            .await?;
    Ok(count)
}

/// Current status, independent of `due()` — a terminal row (e.g.
/// `Delivered`) has `next_transition_at = NULL` and so never appears there.
pub async fn status(pool: &SqlitePool, id: ShipmentId) -> anyhow::Result<Option<ShipmentStatus>> {
    let row: Option<(ShipmentStatus,)> =
        sqlx::query_as("SELECT status FROM shipments WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(status,)| status))
}

pub struct ShipmentRow {
    pub id: ShipmentId,
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub tracking_number: String,
    pub carrier_code: String,
    pub service_code: String,
    pub origin: String,
    pub destination: String,
    pub price_cents: i64,
    pub currency: String,
    pub declared_value_cents: i64,
    pub status: ShipmentStatus,
    pub label_url: Option<String>,
    pub label_format: Option<String>,
    pub eta_at: Option<DateTime<Utc>>,
    pub return_tracking_number: Option<String>,
    pub scenario: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ShipmentSqlRow {
    id: String,
    merchant_id: String,
    provider_slug: String,
    tracking_number: String,
    carrier_code: String,
    service_code: String,
    origin: String,
    destination: String,
    price_cents: i64,
    currency: String,
    declared_value_cents: i64,
    status: ShipmentStatus,
    label_url: Option<String>,
    label_format: Option<String>,
    eta_at: Option<String>,
    return_tracking_number: Option<String>,
    scenario: Option<String>,
    created_at: String,
    updated_at: String,
}

impl ShipmentSqlRow {
    fn into_row(self) -> ShipmentRow {
        ShipmentRow {
            id: self.id.parse().expect("stored shipment id is valid"),
            merchant_id: self
                .merchant_id
                .parse()
                .expect("stored merchant id is valid"),
            provider_slug: self.provider_slug,
            tracking_number: self.tracking_number,
            carrier_code: self.carrier_code,
            service_code: self.service_code,
            origin: self.origin,
            destination: self.destination,
            price_cents: self.price_cents,
            currency: self.currency,
            declared_value_cents: self.declared_value_cents,
            status: self.status,
            label_url: self.label_url,
            label_format: self.label_format,
            eta_at: self.eta_at.as_deref().map(parse_dt),
            return_tracking_number: self.return_tracking_number,
            scenario: self.scenario,
            created_at: parse_dt(&self.created_at),
            updated_at: parse_dt(&self.updated_at),
        }
    }
}

const SHIPMENT_COLUMNS: &str = "id, merchant_id, provider_slug, tracking_number, carrier_code, service_code,
     origin, destination, price_cents, currency, declared_value_cents, status, label_url, label_format,
     eta_at, return_tracking_number, scenario, created_at, updated_at";

pub async fn get(pool: &SqlitePool, id: ShipmentId) -> anyhow::Result<Option<ShipmentRow>> {
    let row: Option<ShipmentSqlRow> = sqlx::query_as(&format!(
        "SELECT {SHIPMENT_COLUMNS} FROM shipments WHERE id = ?1"
    ))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(ShipmentSqlRow::into_row))
}

pub async fn get_by_tracking(
    pool: &SqlitePool,
    provider_slug: &str,
    tracking_number: &str,
) -> anyhow::Result<Option<ShipmentRow>> {
    let row: Option<ShipmentSqlRow> = sqlx::query_as(&format!(
        "SELECT {SHIPMENT_COLUMNS} FROM shipments WHERE provider_slug = ?1 AND tracking_number = ?2"
    ))
    .bind(provider_slug)
    .bind(tracking_number)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(ShipmentSqlRow::into_row))
}

/// See `db::repo::payments::ListFilter`'s doc comment — same optional
/// merchant/provider scoping, for the same reason.
pub struct ListFilter {
    pub merchant_id: Option<MerchantId>,
    pub provider_slug: Option<String>,
    pub status: Option<ShipmentStatus>,
    pub search: Option<String>,
    pub limit: i64,
    pub starting_after: Option<ShipmentId>,
}

pub async fn list(pool: &SqlitePool, filter: &ListFilter) -> anyhow::Result<Vec<ShipmentRow>> {
    let mut sql = format!("SELECT {SHIPMENT_COLUMNS} FROM shipments WHERE 1=1");
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
    if filter.search.is_some() {
        n += 1;
        sql.push_str(&format!(" AND tracking_number LIKE ?{n}"));
    }
    if filter.starting_after.is_some() {
        n += 1;
        sql.push_str(&format!(" AND id < ?{n}"));
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");
    n += 1;
    sql.push_str(&n.to_string());

    let mut query = sqlx::query_as::<_, ShipmentSqlRow>(&sql);
    if let Some(merchant_id) = filter.merchant_id {
        query = query.bind(merchant_id.to_string());
    }
    if let Some(provider_slug) = &filter.provider_slug {
        query = query.bind(provider_slug);
    }
    if let Some(status) = filter.status {
        query = query.bind(status);
    }
    if let Some(search) = &filter.search {
        query = query.bind(format!("%{search}%"));
    }
    if let Some(starting_after) = filter.starting_after {
        query = query.bind(starting_after.to_string());
    }
    query = query.bind(filter.limit);

    let rows = query.fetch_all(pool).await?;
    Ok(rows.into_iter().map(ShipmentSqlRow::into_row).collect())
}

pub async fn set_label(
    pool: &SqlitePool,
    id: ShipmentId,
    label_url: &str,
    label_format: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE shipments SET label_url = ?1, label_format = ?2, updated_at = ?3 WHERE id = ?4",
    )
    .bind(label_url)
    .bind(label_format)
    .bind(now.to_rfc3339())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_return_tracking(
    pool: &SqlitePool,
    id: ShipmentId,
    return_tracking_number: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE shipments SET return_tracking_number = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(return_tracking_number)
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub struct NewRateQuote {
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub origin: String,
    pub destination: String,
    pub packages: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

pub async fn create_rate_quote(
    pool: &SqlitePool,
    new: &NewRateQuote,
) -> anyhow::Result<RateQuoteId> {
    let id = RateQuoteId::new();
    sqlx::query(
        "INSERT INTO rate_quotes (id, merchant_id, provider_slug, order_id, origin, destination, packages, created_at, expires_at)
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(id.to_string())
    .bind(new.merchant_id.to_string())
    .bind(&new.provider_slug)
    .bind(&new.origin)
    .bind(&new.destination)
    .bind(&new.packages)
    .bind(new.created_at.to_rfc3339())
    .bind(new.expires_at.to_rfc3339())
    .execute(pool)
    .await?;
    Ok(id)
}

pub struct NewRateOption {
    pub quote_id: RateQuoteId,
    pub carrier_code: String,
    pub service_code: String,
    pub service_name: String,
    pub amount_cents: i64,
    pub base_cents: i64,
    pub surcharges: String,
    pub currency: String,
    pub eta_min_days: i32,
    pub eta_max_days: i32,
    pub billable_weight_grams: i64,
    pub insurance_cents: i64,
    pub co2_grams: Option<i64>,
}

pub async fn create_rate_option(
    pool: &SqlitePool,
    new: &NewRateOption,
) -> anyhow::Result<RateOptionId> {
    let id = RateOptionId::new();
    sqlx::query(
        "INSERT INTO rate_options (
            id, quote_id, carrier_code, service_code, service_name, amount_cents, base_cents,
            surcharges, currency, eta_min_days, eta_max_days, delivery_window,
            billable_weight_grams, insurance_cents, co2_grams
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12, ?13, ?14)",
    )
    .bind(id.to_string())
    .bind(new.quote_id.to_string())
    .bind(&new.carrier_code)
    .bind(&new.service_code)
    .bind(&new.service_name)
    .bind(new.amount_cents)
    .bind(new.base_cents)
    .bind(&new.surcharges)
    .bind(&new.currency)
    .bind(new.eta_min_days as i64)
    .bind(new.eta_max_days as i64)
    .bind(new.billable_weight_grams)
    .bind(new.insurance_cents)
    .bind(new.co2_grams)
    .execute(pool)
    .await?;
    Ok(id)
}

pub struct RateOptionRow {
    pub id: RateOptionId,
    pub carrier_code: String,
    pub service_code: String,
    pub amount_cents: i64,
    pub currency: String,
    pub billable_weight_grams: i64,
    pub origin: String,
    pub destination: String,
    pub merchant_id: MerchantId,
    pub expires_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct RateOptionSqlRow {
    id: String,
    carrier_code: String,
    service_code: String,
    amount_cents: i64,
    currency: String,
    billable_weight_grams: i64,
    origin: String,
    destination: String,
    merchant_id: String,
    expires_at: String,
}

/// Joins `rate_options` with its parent `rate_quotes` — a shipment created
/// `from a rate_option_id` (spec §11.1) needs the quote's origin/
/// destination, which live on the quote, not the option.
pub async fn get_rate_option(
    pool: &SqlitePool,
    id: RateOptionId,
) -> anyhow::Result<Option<RateOptionRow>> {
    let row: Option<RateOptionSqlRow> = sqlx::query_as(
        "SELECT ro.id, ro.carrier_code, ro.service_code, ro.amount_cents, ro.currency, ro.billable_weight_grams,
                rq.origin, rq.destination, rq.merchant_id, rq.expires_at
         FROM rate_options ro JOIN rate_quotes rq ON rq.id = ro.quote_id
         WHERE ro.id = ?1",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| RateOptionRow {
        id: r.id.parse().expect("stored rate option id is valid"),
        carrier_code: r.carrier_code,
        service_code: r.service_code,
        amount_cents: r.amount_cents,
        currency: r.currency,
        billable_weight_grams: r.billable_weight_grams,
        origin: r.origin,
        destination: r.destination,
        merchant_id: r.merchant_id.parse().expect("stored merchant id is valid"),
        expires_at: parse_dt(&r.expires_at),
    }))
}

pub struct NewPickup {
    pub merchant_id: MerchantId,
    pub provider_slug: String,
    pub address: String,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub shipment_ids: String,
    pub created_at: DateTime<Utc>,
}

pub struct PickupRow {
    pub id: PickupId,
    pub status: String,
    pub confirmation_code: Option<String>,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct PickupSqlRow {
    id: String,
    status: String,
    confirmation_code: Option<String>,
    window_start: String,
    window_end: String,
    created_at: String,
}

pub async fn create_pickup(pool: &SqlitePool, new: &NewPickup) -> anyhow::Result<PickupId> {
    let id = PickupId::new();
    let confirmation_code = format!("PKP{}", &Ulid::new().to_string()[..8]);
    sqlx::query(
        "INSERT INTO pickups (id, merchant_id, provider_slug, address, window_start, window_end, shipment_ids, status, confirmation_code, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'scheduled', ?8, ?9)",
    )
    .bind(id.to_string())
    .bind(new.merchant_id.to_string())
    .bind(&new.provider_slug)
    .bind(&new.address)
    .bind(new.window_start.to_rfc3339())
    .bind(new.window_end.to_rfc3339())
    .bind(&new.shipment_ids)
    .bind(&confirmation_code)
    .bind(new.created_at.to_rfc3339())
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn get_pickup(
    pool: &SqlitePool,
    merchant_id: MerchantId,
    id: PickupId,
) -> anyhow::Result<Option<PickupRow>> {
    let row: Option<PickupSqlRow> = sqlx::query_as(
        "SELECT id, status, confirmation_code, window_start, window_end, created_at
         FROM pickups WHERE id = ?1 AND merchant_id = ?2",
    )
    .bind(id.to_string())
    .bind(merchant_id.to_string())
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| PickupRow {
        id: r.id.parse().expect("stored pickup id is valid"),
        status: r.status,
        confirmation_code: r.confirmation_code,
        window_start: parse_dt(&r.window_start),
        window_end: parse_dt(&r.window_end),
        created_at: parse_dt(&r.created_at),
    }))
}

/// Applies one already-validated transition: updates `shipments` and
/// appends the matching event rows.
#[allow(clippy::too_many_arguments)]
pub async fn advance(
    pool: &SqlitePool,
    id: ShipmentId,
    merchant_id: MerchantId,
    provider_slug: &str,
    to: ShipmentStatus,
    event: EventType,
    seq: i64,
    occurred_at: DateTime<Utc>,
    next_transition_at: Option<DateTime<Utc>>,
    status_reason: Option<&str>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "UPDATE shipments SET status = ?1, status_reason = COALESCE(?2, status_reason), next_transition_at = ?3, updated_at = ?4 WHERE id = ?5",
    )
    .bind(to)
    .bind(status_reason)
    .bind(next_transition_at.map(|t| t.to_rfc3339()))
    .bind(occurred_at.to_rfc3339())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;

    append_event(
        &mut tx,
        id,
        merchant_id,
        provider_slug,
        seq,
        to,
        event,
        occurred_at,
    )
    .await?;

    tx.commit().await?;

    crate::dashboard::activity::publish(crate::dashboard::activity::ActivityEvent::resource(
        "shipment",
        id.to_string(),
        format!("{id} \u{2192} {}", event.description()),
        occurred_at,
    ));

    Ok(())
}

/// `provider_code`/`description` stand in for the dialect-localized text a
/// real provider adapter (M2+) would supply; seed/dashboard polish (M7)
/// can replace these with facility names and localized copy.
#[allow(clippy::too_many_arguments)]
async fn append_event(
    tx: &mut Transaction<'_, Sqlite>,
    shipment_id: ShipmentId,
    merchant_id: MerchantId,
    provider_slug: &str,
    seq: i64,
    status: ShipmentStatus,
    event: EventType,
    occurred_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let occurred_at_text = occurred_at.to_rfc3339();

    sqlx::query(
        "INSERT INTO shipment_events (id, shipment_id, seq, status, provider_code, description, occurred_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
    )
    .bind(Ulid::new().to_string())
    .bind(shipment_id.to_string())
    .bind(seq)
    .bind(status)
    .bind(event.as_str())
    .bind(event.description())
    .bind(&occurred_at_text)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "INSERT INTO events (id, merchant_id, provider_slug, type, resource_type, resource_id, data, created_at)
         VALUES (?1, ?2, ?3, ?4, 'shipment', ?5, ?6, ?7)",
    )
    .bind(EventId::new().to_string())
    .bind(merchant_id.to_string())
    .bind(provider_slug)
    .bind(event.as_str())
    .bind(shipment_id.to_string())
    .bind(format!(r#"{{"status":"{status:?}"}}"#))
    .bind(&occurred_at_text)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::test_support::seed_merchant_and_provider;
    use chrono::Duration;

    #[sqlx::test]
    async fn create_inserts_shipment_and_first_event(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let now = Utc::now();

        let id = create(
            &pool,
            &NewShipment {
                merchant_id: merchant_id.parse().unwrap(),
                provider_slug: provider_slug.clone(),
                carrier_code: "acmeship".into(),
                service_code: "standard".into(),
                origin: "{}".into(),
                destination: "{}".into(),
                tracking_number: "ACM000000000001K".into(),
                price_cents: 1000,
                currency: "USD".into(),
                created_at: now,
                eta_at: now + Duration::hours(24),
            },
        )
        .await
        .unwrap();

        let due_rows = due(&pool, now).await.unwrap();
        assert_eq!(due_rows.len(), 1);
        assert_eq!(due_rows[0].id, id);
        assert_eq!(due_rows[0].status, ShipmentStatus::Created);

        assert_eq!(event_count(&pool, id).await.unwrap(), 1);
    }

    #[sqlx::test]
    async fn advance_updates_status_and_appends_event(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let now = Utc::now();
        let merchant_id: MerchantId = merchant_id.parse().unwrap();

        let id = create(
            &pool,
            &NewShipment {
                merchant_id,
                provider_slug: provider_slug.clone(),
                carrier_code: "acmeship".into(),
                service_code: "standard".into(),
                origin: "{}".into(),
                destination: "{}".into(),
                tracking_number: "ACM000000000002K".into(),
                price_cents: 1000,
                currency: "USD".into(),
                created_at: now,
                eta_at: now + Duration::hours(24),
            },
        )
        .await
        .unwrap();

        advance(
            &pool,
            id,
            merchant_id,
            &provider_slug,
            ShipmentStatus::LabelGenerated,
            EventType::ShipmentLabelGenerated,
            1,
            now,
            Some(now + Duration::hours(2)),
            None,
        )
        .await
        .unwrap();

        let due_rows = due(&pool, now + Duration::hours(2)).await.unwrap();
        assert_eq!(due_rows.len(), 1);
        assert_eq!(due_rows[0].status, ShipmentStatus::LabelGenerated);
        assert_eq!(event_count(&pool, id).await.unwrap(), 2);
    }
}
