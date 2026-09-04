//! Shipment persistence (spec §7.4). Every write here also appends the
//! matching `shipment_events`/`events` rows in the same transaction, so the
//! event log can never drift from `shipments.status`.

use chrono::{DateTime, Utc};
use sqlx::{Sqlite, SqlitePool, Transaction};
use ulid::Ulid;

use crate::domain::event::EventType;
use crate::domain::ids::{EventId, MerchantId, ShipmentId};
use crate::domain::shipment::ShipmentStatus;

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
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "UPDATE shipments SET status = ?1, next_transition_at = ?2, updated_at = ?3 WHERE id = ?4",
    )
    .bind(to)
    .bind(next_transition_at.map(|t| t.to_rfc3339()))
    .bind(occurred_at.to_rfc3339())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;

    append_event(&mut tx, id, merchant_id, provider_slug, seq, to, event, occurred_at).await?;

    tx.commit().await?;
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
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?6)",
    )
    .bind(Ulid::new().to_string())
    .bind(shipment_id.to_string())
    .bind(seq)
    .bind(status)
    .bind(event.as_str())
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
        )
        .await
        .unwrap();

        let due_rows = due(&pool, now + Duration::hours(2)).await.unwrap();
        assert_eq!(due_rows.len(), 1);
        assert_eq!(due_rows[0].status, ShipmentStatus::LabelGenerated);
        assert_eq!(event_count(&pool, id).await.unwrap(), 2);
    }
}
