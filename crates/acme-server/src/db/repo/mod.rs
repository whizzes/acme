//! Repositories: the only code allowed to run `sqlx::query*` (spec §5's
//! rule — nothing under `providers/` may query the database directly).

pub mod shipments;

#[cfg(test)]
pub(crate) mod test_support {
    use chrono::Utc;
    use sqlx::SqlitePool;

    /// Minimal `merchants`/`providers` rows to satisfy the FK constraints
    /// on `shipments`/`payments` in a fresh `#[sqlx::test]` database.
    pub async fn seed_merchant_and_provider(pool: &SqlitePool) -> (String, String) {
        let merchant_id = "mrc_test0000000000000000000".to_string();
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
}
