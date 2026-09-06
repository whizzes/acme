//! Short-lived bearer tokens issued by an OAuth2 grant (specs/006-Dialects.md
//! item 1: Iberex's password grant is the first caller). Wall-clock
//! expiry, like `api_credentials.created_at`/`last_used_at` — this is
//! session bookkeeping, not simulated domain data.

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;
use ulid::Ulid;

use crate::domain::ids::MerchantId;

/// Issues a fresh token for `merchant_id`/`provider_slug`, valid for
/// `ttl`. The token itself is an opaque ULID string, not a prefixed id
/// (spec's own OAuth2 examples show a plain bearer string, not one of
/// `acme`'s `{prefix}_{ulid}` resource ids).
pub async fn issue(
    pool: &SqlitePool,
    merchant_id: MerchantId,
    provider_slug: &str,
    ttl: Duration,
    now: DateTime<Utc>,
) -> anyhow::Result<(String, DateTime<Utc>)> {
    let token = format!("oat_{}", Ulid::new());
    let expires_at = now + ttl;

    sqlx::query(
        "INSERT INTO oauth_tokens (token, merchant_id, provider_slug, expires_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&token)
    .bind(merchant_id.to_string())
    .bind(provider_slug)
    .bind(expires_at.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;

    Ok((token, expires_at))
}

/// `Some(merchant_id)` when `token` exists, matches `provider_slug`, and
/// hasn't expired — the shape `http::auth::oauth2_bearer_auth` needs to
/// resolve a request the same way `bearer_auth` resolves a static one.
pub async fn validate(
    pool: &SqlitePool,
    token: &str,
    provider_slug: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<Option<MerchantId>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT merchant_id FROM oauth_tokens
         WHERE token = ?1 AND provider_slug = ?2 AND expires_at > ?3",
    )
    .bind(token)
    .bind(provider_slug)
    .bind(now.to_rfc3339())
    .fetch_optional(pool)
    .await?;

    Ok(row.and_then(|(id,)| id.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::test_support::seed_merchant_and_provider;

    #[sqlx::test]
    async fn issued_token_validates_until_it_expires(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();

        let (token, _expires_at) =
            issue(&pool, merchant_id, &provider_slug, Duration::hours(1), now)
                .await
                .unwrap();

        assert_eq!(
            validate(&pool, &token, &provider_slug, now).await.unwrap(),
            Some(merchant_id)
        );
        assert_eq!(
            validate(&pool, &token, &provider_slug, now + Duration::hours(2))
                .await
                .unwrap(),
            None,
            "an expired token must not validate"
        );
        assert_eq!(
            validate(&pool, &token, "some-other-provider", now)
                .await
                .unwrap(),
            None,
            "a token must not validate for a different provider"
        );
        assert_eq!(
            validate(&pool, "oat_not_a_real_token", &provider_slug, now)
                .await
                .unwrap(),
            None
        );
    }
}
