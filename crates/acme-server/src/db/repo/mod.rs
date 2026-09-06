//! Repositories: the only code allowed to run `sqlx::query*` (spec §5's
//! rule — nothing under `providers/` may query the database directly).

pub mod dashboard;
pub mod events;
pub mod payments;
pub mod shipments;
pub mod traffic;
pub mod webhooks;

/// Non-`#[cfg(test)]` so integration tests under `tests/` (which link
/// `acme_server` as an external crate) can reach it too.
pub mod test_support {
    use chrono::Utc;
    use sqlx::SqlitePool;

    /// Minimal `merchants`/`providers` rows to satisfy the FK constraints
    /// on `shipments`/`payments` in a fresh `#[sqlx::test]` database.
    pub async fn seed_merchant_and_provider(pool: &SqlitePool) -> (String, String) {
        let merchant_id = "mrc_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string();
        let provider_slug = "acmeship".to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO merchants (id, name, country, default_currency, created_at)
             VALUES (?1, 'Test Merchant', 'CL', 'CLP', ?2)",
        )
        .bind(&merchant_id)
        .bind(&now)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO providers (slug, kind, display_name, dialect, base_path, auth_scheme, countries, currencies, capabilities)
             VALUES (?1, 'shipping', 'Acme Ship', 'house', '/acmeship/v1', 'bearer', '[]', '[]', '[]')",
        )
        .bind(&provider_slug)
        .execute(pool)
        .await
        .unwrap();

        (merchant_id, provider_slug)
    }

    /// Well-known secret keys for the seeded merchant below, matching the
    /// `sk_test_…` shape spec §10.1's example request uses.
    pub const ACMEPAY_SECRET_KEY: &str = "sk_test_acmepay_demo0000000000";
    pub const ACMESHIP_SECRET_KEY: &str = "sk_test_acmeship_demo000000000";
    pub const MERCHANT_ID: &str = "mrc_01ARZ3NDEKTSV4RRFFQ69G5FAX";

    /// One merchant with both reference-dialect providers registered and
    /// one active API credential each — the fixture `tests/scenarios.rs`
    /// and the provider integration tests authenticate against.
    pub async fn seed_reference_merchant(pool: &SqlitePool) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO merchants (id, name, country, default_currency, created_at)
             VALUES (?1, 'Acme Test Merchant', 'ES', 'EUR', ?2)",
        )
        .bind(MERCHANT_ID)
        .bind(&now)
        .execute(pool)
        .await?;

        sqlx::query(
            "INSERT INTO providers (slug, kind, display_name, dialect, base_path, auth_scheme, countries, currencies, capabilities)
             VALUES
                ('acmepay', 'payment', 'Acme Pay', 'house', '/acmepay/v1', 'bearer', '[]', '[]', '[]'),
                ('acmeship', 'shipping', 'Acme Ship', 'house', '/acmeship/v1', 'bearer', '[]', '[]', '[]')",
        )
        .execute(pool)
        .await?;

        for (slug, secret) in [
            ("acmepay", ACMEPAY_SECRET_KEY),
            ("acmeship", ACMESHIP_SECRET_KEY),
        ] {
            sqlx::query(
                "INSERT INTO api_credentials (id, merchant_id, provider_slug, label, secret_key, active, created_at)
                 VALUES (?1, ?2, ?3, 'default', ?4, 1, ?5)",
            )
            .bind(format!("cred_{slug}"))
            .bind(MERCHANT_ID)
            .bind(slug)
            .bind(secret)
            .bind(&now)
            .execute(pool)
            .await?;
        }

        Ok(())
    }
}
