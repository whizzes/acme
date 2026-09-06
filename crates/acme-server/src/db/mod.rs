//! SQLite pool setup: pragmas applied per-connection, then migrations run.

pub mod repo;

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::config::Config;
use crate::sim::clock::SimClock;

/// Opens the pool and runs pending migrations.
pub async fn connect(database_url: &str) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_millis(5000))
        .pragma("temp_store", "MEMORY");

    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .connect_with(opts)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}

/// Seeds `sim_settings` from `Config` on first boot only, then builds a
/// `SimClock` from whatever is on the row — env vars only provide the
/// *initial* values (spec §6); a restart never resets a dashboard-adjusted
/// clock.
pub async fn bootstrap_sim_clock(pool: &SqlitePool, cfg: &Config) -> anyhow::Result<SimClock> {
    let existing: Option<(String, f64)> =
        sqlx::query_as("SELECT clock_epoch, multiplier FROM sim_settings WHERE id = 1")
            .fetch_optional(pool)
            .await?;

    if let Some((epoch_text, multiplier)) = existing {
        let epoch: DateTime<Utc> = DateTime::parse_from_rfc3339(&epoch_text)?.with_timezone(&Utc);
        return Ok(SimClock::new(epoch, multiplier));
    }

    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO sim_settings (id, clock_epoch, clock_started_at, multiplier, latency_ms, failure_rate, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?2)",
    )
    .bind(cfg.clock_epoch.to_rfc3339())
    .bind(&now)
    .bind(cfg.clock_multiplier)
    .bind(cfg.latency_ms as i64)
    .bind(cfg.failure_rate)
    .execute(pool)
    .await?;

    Ok(SimClock::new(cfg.clock_epoch, cfg.clock_multiplier))
}

/// Demo API credentials, seeded on first boot so Swagger UI's "Try it
/// out" works without a separate seeding step (spec §14: "pre-filled
/// sandbox credentials... a newcomer can call an endpoint within ten
/// seconds of landing"). The full faker-driven world is M7's `seed`
/// subcommand; this is only the one merchant + one credential per
/// reference provider that M2's own demo needs.
pub const DEMO_MERCHANT_ID: &str = "mrc_01ARZ3NDEKTSV4RRFFQ69G5FAV";
pub const DEMO_ACMEPAY_SECRET_KEY: &str = "sk_test_acmepay_demo";
pub const DEMO_ACMESHIP_SECRET_KEY: &str = "sk_test_acmeship_demo";
/// Trancorp Webpay's header key-pair (spec §10.2's own worked example
/// shape — a numeric key id, a long hex secret).
pub const DEMO_WEBPAY_KEY_ID: &str = "597055555532";
pub const DEMO_WEBPAY_KEY_SECRET: &str =
    "579B532A7440BB0C9079DED94D31EA1615BACEB56610332264630D42D0A36B1C";
/// Iberex's OAuth2 password-grant credentials.
pub const DEMO_IBEREX_CLIENT_ID: &str = "iberex_client_demo";
pub const DEMO_IBEREX_CLIENT_SECRET: &str = "iberex_secret_demo";
pub const DEMO_IBEREX_USERNAME: &str = "iberex_demo";
pub const DEMO_IBEREX_PASSWORD: &str = "iberex_demo_pw";

pub async fn bootstrap_demo_credentials(pool: &SqlitePool) -> anyhow::Result<()> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM merchants")
        .fetch_one(pool)
        .await?;
    if count > 0 {
        return Ok(());
    }

    let now = Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO merchants (id, name, country, default_currency, created_at)
         VALUES (?1, 'Acme Demo Merchant', 'CL', 'CLP', ?2)",
    )
    .bind(DEMO_MERCHANT_ID)
    .bind(&now)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO providers (slug, kind, display_name, dialect, base_path, auth_scheme, countries, currencies, capabilities)
         VALUES
            ('acmepay', 'payment', 'Acme Pay', 'house', '/acmepay/v1', 'bearer', '[]', '[]', '[]'),
            ('acmeship', 'shipping', 'Acme Ship', 'house', '/acmeship/v1', 'bearer', '[]', '[]', '[]'),
            ('webpay', 'payment', 'Trancorp Webpay', 'transbank', '/rswebpaytransaction/api/webpay/v1.2', 'header_key_pair', '[\"CL\"]', '[\"CLP\"]', '[]'),
            ('iberex', 'shipping', 'Iberex Express', 'seur', '/iberex', 'oauth2_password', '[\"ES\"]', '[\"EUR\"]', '[]')",
    )
    .execute(pool)
    .await?;

    for (slug, secret) in [
        ("acmepay", DEMO_ACMEPAY_SECRET_KEY),
        ("acmeship", DEMO_ACMESHIP_SECRET_KEY),
    ] {
        sqlx::query(
            "INSERT INTO api_credentials (id, merchant_id, provider_slug, label, secret_key, active, created_at)
             VALUES (?1, ?2, ?3, 'demo', ?4, 1, ?5)",
        )
        .bind(format!("cred_demo_{slug}"))
        .bind(DEMO_MERCHANT_ID)
        .bind(slug)
        .bind(secret)
        .bind(&now)
        .execute(pool)
        .await?;
    }

    // Webpay: `public_key`/`secret_key` hold the `Tbk-Api-Key-Id`/
    // `Tbk-Api-Key-Secret` pair `http::auth::header_key_pair_auth` checks.
    sqlx::query(
        "INSERT INTO api_credentials (id, merchant_id, provider_slug, label, public_key, secret_key, active, created_at)
         VALUES (?1, ?2, 'webpay', 'demo', ?3, ?4, 1, ?5)",
    )
    .bind("cred_demo_webpay")
    .bind(DEMO_MERCHANT_ID)
    .bind(DEMO_WEBPAY_KEY_ID)
    .bind(DEMO_WEBPAY_KEY_SECRET)
    .bind(&now)
    .execute(pool)
    .await?;

    // Iberex: `public_key`/`secret_key` hold `client_id`/`client_secret`;
    // `extra` carries the `username`/`password` pair the password grant
    // also requires, as JSON — `api_credentials.extra` is untyped TEXT for
    // exactly this kind of dialect-specific extra field (spec §7.2).
    sqlx::query(
        "INSERT INTO api_credentials (id, merchant_id, provider_slug, label, public_key, secret_key, extra, active, created_at)
         VALUES (?1, ?2, 'iberex', 'demo', ?3, ?4, ?5, 1, ?6)",
    )
    .bind("cred_demo_iberex")
    .bind(DEMO_MERCHANT_ID)
    .bind(DEMO_IBEREX_CLIENT_ID)
    .bind(DEMO_IBEREX_CLIENT_SECRET)
    .bind(
        serde_json::json!({ "username": DEMO_IBEREX_USERNAME, "password": DEMO_IBEREX_PASSWORD })
            .to_string(),
    )
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(())
}
