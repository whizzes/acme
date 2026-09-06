//! `sim_settings` read/write (spec §8.3/§9.3, specs/008-Simulation.md
//! item 3): the persistence half of the clock and the global latency/
//! failure sliders, both of which have been settable in memory
//! (`sim::clock::SimClock`) since M1 with nothing writing the change back
//! — `db::bootstrap_sim_clock`'s own doc comment promised "a restart
//! never resets a dashboard-adjusted clock"; this module is what makes
//! that true.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

pub struct SimSettingsRow {
    pub clock_epoch: DateTime<Utc>,
    pub multiplier: f64,
    pub paused: bool,
    pub latency_ms: i64,
    pub failure_rate: f64,
    pub webhook_failure_rate: f64,
    pub updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct SimSettingsSqlRow {
    clock_epoch: String,
    multiplier: f64,
    paused: i64,
    latency_ms: i64,
    failure_rate: f64,
    webhook_failure_rate: f64,
    updated_at: String,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .expect("stored timestamps are valid rfc3339")
        .with_timezone(&Utc)
}

impl SimSettingsSqlRow {
    fn into_row(self) -> SimSettingsRow {
        SimSettingsRow {
            clock_epoch: parse_dt(&self.clock_epoch),
            multiplier: self.multiplier,
            paused: self.paused != 0,
            latency_ms: self.latency_ms,
            failure_rate: self.failure_rate,
            webhook_failure_rate: self.webhook_failure_rate,
            updated_at: parse_dt(&self.updated_at),
        }
    }
}

/// `sim_settings` always has exactly one row (`id = 1`, spec §7.5's own
/// `CHECK (id = 1)`), written by `db::bootstrap_sim_clock` on first boot
/// — this never returns `None` in a running system.
pub async fn get(pool: &SqlitePool) -> anyhow::Result<SimSettingsRow> {
    let row: SimSettingsSqlRow = sqlx::query_as(
        "SELECT clock_epoch, multiplier, paused, latency_ms, failure_rate, webhook_failure_rate, updated_at
         FROM sim_settings WHERE id = 1",
    )
    .fetch_one(pool)
    .await?;
    Ok(row.into_row())
}

/// Persists the clock's current sim time as the new `clock_epoch` — every
/// clock mutation (speed change, pause, resume, jump) calls this
/// immediately after mutating the live `SimClock`, so a restart
/// reconstructs the same moment `db::bootstrap_sim_clock` would have
/// found had the process never stopped. `multiplier` is the *resume-to*
/// speed, not necessarily the clock's current effective rate — see
/// specs/008-Simulation.md's own Caveat on why pausing writes `paused =
/// 1` without touching this value.
pub async fn save_clock(
    pool: &SqlitePool,
    clock_epoch: DateTime<Utc>,
    multiplier: f64,
    paused: bool,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE sim_settings SET clock_epoch = ?1, clock_started_at = ?2, multiplier = ?3, paused = ?4, updated_at = ?2
         WHERE id = 1",
    )
    .bind(clock_epoch.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(multiplier)
    .bind(paused)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn save_latency_and_failure(
    pool: &SqlitePool,
    latency_ms: i64,
    failure_rate: f64,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE sim_settings SET latency_ms = ?1, failure_rate = ?2, updated_at = ?3 WHERE id = 1",
    )
    .bind(latency_ms)
    .bind(failure_rate)
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Clears every dynamic table for the Simulator's "Reset and reseed"
/// (spec §13.6, specs/008-Simulation.md item 6) — everything the demo has
/// accumulated, so `db::bootstrap_demo_credentials` (which only seeds
/// when `merchants` is empty) can run again. Deliberately leaves
/// `sim_settings` untouched: a reset clears *data*, not the operator's
/// clock/latency/failure configuration. Children deleted before parents
/// so this is correct regardless of which FKs actually cascade.
pub async fn reset_dynamic_tables(pool: &SqlitePool) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    for table in [
        "exchange_resources",
        "http_exchanges",
        "http_bodies",
        "webhook_deliveries",
        "webhook_endpoints",
        "payment_events",
        "refunds",
        "checkout_sessions",
        "card_tokens",
        "payments",
        "shipment_events",
        "rate_options",
        "rate_quotes",
        "pickups",
        "shipments",
        "events",
        "oauth_tokens",
        "sim_faults",
        "idempotency_keys",
        "api_credentials",
        "providers",
        "merchants",
    ] {
        sqlx::query(&format!("DELETE FROM {table}"))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    async fn seed_settings(pool: &SqlitePool) {
        crate::db::bootstrap_sim_clock(pool, &Config::default())
            .await
            .unwrap();
    }

    #[sqlx::test]
    async fn save_clock_round_trips(pool: SqlitePool) {
        seed_settings(&pool).await;
        let now = Utc::now();
        save_clock(&pool, now, 120.0, true, now).await.unwrap();

        let row = get(&pool).await.unwrap();
        assert_eq!(row.multiplier, 120.0);
        assert!(row.paused);
        assert_eq!(row.clock_epoch.timestamp(), now.timestamp());
    }

    #[sqlx::test]
    async fn save_latency_and_failure_round_trips(pool: SqlitePool) {
        seed_settings(&pool).await;
        save_latency_and_failure(&pool, 250, 0.1, Utc::now())
            .await
            .unwrap();

        let row = get(&pool).await.unwrap();
        assert_eq!(row.latency_ms, 250);
        assert_eq!(row.failure_rate, 0.1);
    }
}
