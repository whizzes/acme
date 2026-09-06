//! Read side of the shared `events` log (spec §7.5) — written by
//! `db::repo::payments`/`db::repo::shipments`, read here by both
//! providers' `GET /events` endpoints.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::domain::ids::{EventId, MerchantId};

pub struct EventRow {
    pub id: EventId,
    pub kind: String,
    pub resource_type: String,
    pub resource_id: String,
    pub data: String,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct EventSqlRow {
    id: String,
    #[sqlx(rename = "type")]
    kind: String,
    resource_type: String,
    resource_id: String,
    data: String,
    created_at: String,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .expect("stored timestamps are valid rfc3339")
        .with_timezone(&Utc)
}

impl EventSqlRow {
    fn into_row(self) -> EventRow {
        EventRow {
            id: self.id.parse().expect("stored event id is valid"),
            kind: self.kind,
            resource_type: self.resource_type,
            resource_id: self.resource_id,
            data: self.data,
            created_at: parse_dt(&self.created_at),
        }
    }
}

/// Cursor-paginated, newest first — `starting_after` is the previous
/// page's last id (event ids are ULIDs, so lexical order is creation
/// order).
pub async fn list(
    pool: &SqlitePool,
    merchant_id: MerchantId,
    provider_slug: &str,
    limit: i64,
    starting_after: Option<EventId>,
) -> anyhow::Result<Vec<EventRow>> {
    let rows: Vec<EventSqlRow> = if let Some(cursor) = starting_after {
        sqlx::query_as(
            "SELECT id, type, resource_type, resource_id, data, created_at FROM events
             WHERE merchant_id = ?1 AND provider_slug = ?2 AND id < ?3 ORDER BY id DESC LIMIT ?4",
        )
        .bind(merchant_id.to_string())
        .bind(provider_slug)
        .bind(cursor.to_string())
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, type, resource_type, resource_id, data, created_at FROM events
             WHERE merchant_id = ?1 AND provider_slug = ?2 ORDER BY id DESC LIMIT ?3",
        )
        .bind(merchant_id.to_string())
        .bind(provider_slug)
        .bind(limit)
        .fetch_all(pool)
        .await?
    };

    Ok(rows.into_iter().map(EventSqlRow::into_row).collect())
}

pub async fn get(
    pool: &SqlitePool,
    merchant_id: MerchantId,
    id: EventId,
) -> anyhow::Result<Option<EventRow>> {
    let row: Option<EventSqlRow> = sqlx::query_as(
        "SELECT id, type, resource_type, resource_id, data, created_at FROM events WHERE id = ?1 AND merchant_id = ?2",
    )
    .bind(id.to_string())
    .bind(merchant_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(EventSqlRow::into_row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::payments::{self, NewPayment};
    use crate::db::repo::test_support::seed_merchant_and_provider;

    #[sqlx::test]
    async fn list_returns_events_written_by_payment_create(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        payments::create(
            &pool,
            &NewPayment {
                merchant_id,
                provider_slug: provider_slug.clone(),
                amount_cents: 100,
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

        let events = list(&pool, merchant_id, &provider_slug, 10, None)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].resource_type, "payment");
    }
}
