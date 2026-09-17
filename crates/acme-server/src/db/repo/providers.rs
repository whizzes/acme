//! Read side of the `providers` catalog (spec §13.3's `/providers` and
//! `/providers/{slug}`, deferred from M3 by specs/004-Dashboard.md,
//! finally worth building in specs/006-Dialects.md item 5 now that more
//! than two providers exist).

use sqlx::SqlitePool;

use crate::domain::ids::MerchantId;

pub struct ProviderRow {
    pub slug: String,
    pub kind: String,
    pub display_name: String,
    pub dialect: String,
    pub base_path: String,
    pub auth_scheme: String,
    pub enabled: bool,
}

#[derive(sqlx::FromRow)]
struct ProviderSqlRow {
    slug: String,
    kind: String,
    display_name: String,
    dialect: String,
    base_path: String,
    auth_scheme: String,
    enabled: i64,
}

impl ProviderSqlRow {
    fn into_row(self) -> ProviderRow {
        ProviderRow {
            slug: self.slug,
            kind: self.kind,
            display_name: self.display_name,
            dialect: self.dialect,
            base_path: self.base_path,
            auth_scheme: self.auth_scheme,
            enabled: self.enabled != 0,
        }
    }
}

pub async fn list(pool: &SqlitePool) -> anyhow::Result<Vec<ProviderRow>> {
    let rows: Vec<ProviderSqlRow> = sqlx::query_as(
        "SELECT slug, kind, display_name, dialect, base_path, auth_scheme, enabled
         FROM providers ORDER BY kind, slug",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(ProviderSqlRow::into_row).collect())
}

pub async fn get(pool: &SqlitePool, slug: &str) -> anyhow::Result<Option<ProviderRow>> {
    let row: Option<ProviderSqlRow> = sqlx::query_as(
        "SELECT slug, kind, display_name, dialect, base_path, auth_scheme, enabled
         FROM providers WHERE slug = ?1",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(ProviderSqlRow::into_row))
}

/// One credential shown on `/providers/{slug}` (spec §18: this is a
/// sandbox, secrets are revealed). `public_key` is exposed alongside
/// `secret_key` since some auth schemes (spec §10.2's header key-pair,
/// §11.2's OAuth2 client credentials) need both to authenticate.
pub struct CredentialRow {
    pub merchant_id: MerchantId,
    pub label: Option<String>,
    pub public_key: Option<String>,
    pub secret_key: String,
}

pub async fn demo_credential(
    pool: &SqlitePool,
    provider_slug: &str,
) -> anyhow::Result<Option<CredentialRow>> {
    let row: Option<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT merchant_id, label, public_key, secret_key FROM api_credentials
         WHERE provider_slug = ?1 ORDER BY created_at ASC LIMIT 1",
    )
    .bind(provider_slug)
    .fetch_optional(pool)
    .await?;
    row.map(|(merchant_id, label, public_key, secret_key)| {
        Ok(CredentialRow {
            merchant_id: merchant_id.parse()?,
            label,
            public_key,
            secret_key,
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::test_support::seed_merchant_and_provider;

    #[sqlx::test]
    async fn list_and_get_return_seeded_providers(pool: SqlitePool) {
        seed_merchant_and_provider(&pool).await;

        let all = list(&pool).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].slug, "acmeship");

        let one = get(&pool, "acmeship").await.unwrap().unwrap();
        assert_eq!(one.kind, "shipping");
        assert!(get(&pool, "nonexistent").await.unwrap().is_none());
    }
}
