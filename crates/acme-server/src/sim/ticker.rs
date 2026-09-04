//! Background task (spec §4): once per wall second, advances every due
//! payment/shipment and emits the resulting events.

use std::hash::{Hash, Hasher};
use std::time::Duration as StdDuration;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::db::repo::shipments;
use crate::domain::shipment::{self, happy_path_schedule};
use crate::sim::clock::SimClock;

/// Deterministic per-shipment RNG seed, so the same shipment always
/// regenerates the same happy-path schedule across ticks (the schedule
/// itself is never persisted — see `db::repo::shipments::event_count`).
fn seed_from_id(id: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

/// Advances every due shipment by exactly one happy-path hop, or does
/// nothing for a row whose schedule is already exhausted (delivered).
/// Returns how many rows were advanced. Called directly (not just via
/// `spawn`) by tests that want to drive the ticker without a real
/// background task.
pub async fn tick_shipments(pool: &SqlitePool, clock: &SimClock) -> anyhow::Result<usize> {
    let now = clock.now();
    let due = shipments::due(pool, now).await?;
    let mut advanced = 0;

    for row in due {
        let Some(eta_at) = row.eta_at else {
            tracing::warn!(id = %row.id, "shipment has no eta_at, skipping");
            continue;
        };
        let eta = eta_at - row.created_at;
        if eta <= chrono::Duration::zero() {
            tracing::warn!(id = %row.id, "shipment eta_at is not after created_at, skipping");
            continue;
        }

        let schedule = happy_path_schedule(eta, seed_from_id(&row.id.to_string()));
        let seq = shipments::event_count(pool, row.id).await?;
        let Some(hop) = schedule.get(seq as usize) else {
            continue;
        };

        let transition = shipment::apply(row.status, hop.command, clock)?;
        let occurred_at = row.created_at + hop.at;
        let next_at = schedule.get(seq as usize + 1).map(|h| row.created_at + h.at);

        shipments::advance(
            pool,
            row.id,
            row.merchant_id,
            &row.provider_slug,
            transition.to,
            transition.event,
            seq,
            occurred_at,
            next_at,
        )
        .await?;

        advanced += 1;
    }

    Ok(advanced)
}

/// Spawns the ticker loop; cancels cleanly on `token.cancel()`.
pub fn spawn(pool: SqlitePool, clock: SimClock, token: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(StdDuration::from_secs(1));
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = tick_shipments(&pool, &clock).await {
                        tracing::error!(?error, "ticker: shipment tick failed");
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::shipments::NewShipment;
    use crate::db::repo::test_support::seed_merchant_and_provider;
    use crate::domain::shipment::ShipmentStatus;
    use chrono::{Duration, Utc};

    /// Drives the ticker step by step under a clock parked far in the
    /// future (so every hop is immediately due, independent of wall time)
    /// until the shipment reaches `Delivered`, then checks the event log
    /// has one row per hop: `Created`, `LabelGenerated`, and the seven
    /// happy-path fractions from `domain::shipment::happy_path_schedule`.
    #[sqlx::test]
    async fn ticker_advances_a_shipment_to_delivered(pool: SqlitePool) {
        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let now = Utc::now();
        let eta = Duration::hours(25) + Duration::minutes(40);

        let id = shipments::create(
            &pool,
            &NewShipment {
                merchant_id: merchant_id.parse().unwrap(),
                provider_slug,
                carrier_code: "iberex".into(),
                service_code: "express_10h".into(),
                origin: "{}".into(),
                destination: "{}".into(),
                tracking_number: "ACM000000000003K".into(),
                price_cents: 5000,
                currency: "EUR".into(),
                created_at: now,
                eta_at: now + eta,
            },
        )
        .await
        .unwrap();

        let clock = SimClock::new(now + eta + Duration::days(1), 0.0);

        let mut delivered = false;
        for _ in 0..32 {
            let advanced = tick_shipments(&pool, &clock).await.unwrap();
            if shipments::status(&pool, id).await.unwrap() == Some(ShipmentStatus::Delivered) {
                delivered = true;
                break;
            }
            if advanced == 0 {
                break;
            }
        }

        assert!(delivered, "shipment never reached Delivered");
        assert_eq!(shipments::event_count(&pool, id).await.unwrap(), 9);
    }
}
