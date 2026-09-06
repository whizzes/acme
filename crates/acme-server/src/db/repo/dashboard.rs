//! Aggregate counters and sparkline buckets for the Overview page (spec
//! §13.2's KPI row) — a cross-merchant, cross-provider read, unlike the
//! provider-facing `db::repo::events::list` (specs/004-Dashboard.md's
//! Caveats explain why this is a new module rather than loosening that
//! one's scoping contract).

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

pub struct Counters {
    pub payments_total: i64,
    pub payments_captured: i64,
    pub shipments_total: i64,
    pub shipments_delivered: i64,
    pub webhook_endpoints_total: i64,
}

pub async fn counters(pool: &SqlitePool) -> anyhow::Result<Counters> {
    let (payments_total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM payments")
        .fetch_one(pool)
        .await?;
    let (payments_captured,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM payments WHERE status = 'captured'")
            .fetch_one(pool)
            .await?;
    let (shipments_total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM shipments")
        .fetch_one(pool)
        .await?;
    let (shipments_delivered,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM shipments WHERE status = 'delivered'")
            .fetch_one(pool)
            .await?;
    let (webhook_endpoints_total,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM webhook_endpoints")
            .fetch_one(pool)
            .await?;

    Ok(Counters {
        payments_total,
        payments_captured,
        shipments_total,
        shipments_delivered,
        webhook_endpoints_total,
    })
}

pub struct HourlyPoint {
    /// `YYYY-MM-DDTHH:00:00`, sim time — the same clock the sparkline's
    /// window is measured against.
    pub hour: String,
    pub count: i64,
}

/// One point per hour that has at least one capture, since `since`
/// (sim time). Callers align this against a fixed hour range themselves
/// (spec §13.2's mockup shows a fixed "last 6h" window).
pub async fn payments_captured_by_hour(
    pool: &SqlitePool,
    since: DateTime<Utc>,
) -> anyhow::Result<Vec<HourlyPoint>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT strftime('%Y-%m-%dT%H:00:00', captured_at) AS hour, COUNT(*) FROM payments
         WHERE status = 'captured' AND captured_at IS NOT NULL AND captured_at >= ?1
         GROUP BY hour ORDER BY hour ASC",
    )
    .bind(since.to_rfc3339())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(hour, count)| HourlyPoint { hour, count })
        .collect())
}

pub async fn shipments_delivered_by_hour(
    pool: &SqlitePool,
    since: DateTime<Utc>,
) -> anyhow::Result<Vec<HourlyPoint>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT strftime('%Y-%m-%dT%H:00:00', updated_at) AS hour, COUNT(*) FROM shipments
         WHERE status = 'delivered' AND updated_at >= ?1
         GROUP BY hour ORDER BY hour ASC",
    )
    .bind(since.to_rfc3339())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(hour, count)| HourlyPoint { hour, count })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::payments::{self, NewPayment};
    use crate::db::repo::test_support::seed_merchant_and_provider;
    use crate::domain::event::EventType;
    use crate::domain::ids::MerchantId;
    use crate::domain::payment::PaymentStatus;

    #[sqlx::test]
    async fn counters_reflect_seeded_rows(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        let id = payments::create(
            &pool,
            &NewPayment {
                merchant_id,
                provider_slug: provider_slug.clone(),
                amount_cents: 1000,
                currency: "USD".into(),
                capture_mode: "automatic".into(),
                reference: None,
                method_kind: "card".into(),
                method_detail: None,
                installments: 1,
                scenario: "approve".into(),
                risk_score: None,
                risk_decision: None,
                three_ds: None,
                metadata: None,
                created_at: now,
            },
        )
        .await
        .unwrap();
        payments::advance(
            &pool,
            id,
            merchant_id,
            &provider_slug,
            PaymentStatus::Captured,
            EventType::PaymentCaptured,
            1,
            now,
            None,
            None,
        )
        .await
        .unwrap();

        let counters = counters(&pool).await.unwrap();
        assert_eq!(counters.payments_total, 1);
        assert_eq!(counters.payments_captured, 1);
        assert_eq!(counters.shipments_total, 0);

        let series = payments_captured_by_hour(&pool, now - chrono::Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(series.iter().map(|p| p.count).sum::<i64>(), 1);
    }
}
