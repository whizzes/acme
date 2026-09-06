//! Webhook endpoint registration (spec §7.5, §10.1/§11.1's `webhook_endpoints`
//! CRUD). Delivery itself — the dispatcher, signing, retries — is M4; this
//! milestone only persists the registration a caller makes.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::domain::ids::{MerchantId, WebhookEndpointId};

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
    description: Option<String>,
    created_at: String,
}

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
    let row: Option<WebhookEndpointSqlRow> = sqlx::query_as(
        "SELECT id, merchant_id, provider_slug, url, enabled_events, active, description, created_at
         FROM webhook_endpoints WHERE id = ?1 AND merchant_id = ?2",
    )
    .bind(id.to_string())
    .bind(merchant_id.to_string())
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
}
