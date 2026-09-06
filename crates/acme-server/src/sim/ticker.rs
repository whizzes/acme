//! Background task (spec §4): once per wall second, advances every due
//! payment/shipment and emits the resulting events.

use std::hash::{Hash, Hasher};
use std::time::Duration as StdDuration;

use std::str::FromStr;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::db::repo::{payments, shipments};
use crate::domain::scenario::{self, PaymentScenario};
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
        let next_at = schedule
            .get(seq as usize + 1)
            .map(|h| row.created_at + h.at);

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
            None,
        )
        .await?;

        advanced += 1;
    }

    Ok(advanced)
}

/// Advances every due scenario-driven payment by exactly one step (spec
/// §9.1's delayed outcomes: `slow_approval`, `manual_review`,
/// `three_ds_challenge`/`_fail`, `chargeback`), via
/// `domain::scenario::payment_next_step` rather than a precomputed
/// schedule — payment scenarios are at most two hops past `created`, so
/// there's no schedule worth generating up front (contrast
/// `shipment::happy_path_schedule`). Returns how many rows were advanced.
pub async fn tick_payments(pool: &SqlitePool, clock: &SimClock) -> anyhow::Result<usize> {
    let now = clock.now();
    let due = payments::due(pool, now).await?;
    let mut advanced = 0;

    for row in due {
        let Some(scenario_str) = row.scenario.as_deref() else {
            continue;
        };
        let Ok(scenario) = PaymentScenario::from_str(scenario_str) else {
            continue;
        };
        let Some((_delay, command)) = scenario::payment_next_step(scenario, row.status) else {
            continue;
        };

        let transition = crate::domain::payment::apply(row.status, command, clock)?;
        let seq = payments::event_count(pool, row.id).await?;
        let next_at =
            scenario::payment_next_step(scenario, transition.to).map(|(delay, _)| now + delay);

        payments::advance(
            pool,
            row.id,
            row.merchant_id,
            &row.provider_slug,
            transition.to,
            transition.event,
            seq,
            transition.occurred_at,
            next_at,
            None,
        )
        .await?;

        advanced += 1;
    }

    Ok(advanced)
}

/// Spawns the ticker loop; cancels cleanly on `token.cancel()`.
pub fn spawn(
    pool: SqlitePool,
    clock: SimClock,
    token: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(StdDuration::from_secs(1));
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = tick_shipments(&pool, &clock).await {
                        tracing::error!(?error, "ticker: shipment tick failed");
                    }
                    if let Err(error) = tick_payments(&pool, &clock).await {
                        tracing::error!(?error, "ticker: payment tick failed");
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

    /// A `slow_approval` payment (spec §9.1: "pending for 3 sim minutes,
    /// then captured") starts `Pending` at creation, gets a
    /// `next_transition_at`, and the ticker walks it the rest of the way —
    /// mirroring the shipment test above, but for `tick_payments`.
    #[sqlx::test]
    async fn ticker_advances_a_slow_approval_payment_to_captured(pool: SqlitePool) {
        use crate::domain::ids::MerchantId;
        use crate::domain::payment::{self, PaymentCommand, PaymentStatus};

        let (merchant_id, provider_slug) = seed_merchant_and_provider(&pool).await;
        let merchant_id: MerchantId = merchant_id.parse().unwrap();
        let now = Utc::now();
        let clock = SimClock::new(now, 0.0);

        let id = payments::create(
            &pool,
            &payments::NewPayment {
                merchant_id,
                provider_slug: provider_slug.clone(),
                amount_cents: 10_000,
                currency: "USD".into(),
                capture_mode: "automatic".into(),
                reference: None,
                method_kind: "card".into(),
                method_detail: None,
                installments: 1,
                scenario: "slow_approval".into(),
                risk_score: None,
                risk_decision: None,
                three_ds: None,
                metadata: None,
                created_at: now,
            },
        )
        .await
        .unwrap();

        // What `create_payment`'s handler does synchronously: apply the
        // zero-delay `MarkPending` step, then schedule the delayed one.
        let transition =
            payment::apply(PaymentStatus::Created, PaymentCommand::MarkPending, &clock).unwrap();
        payments::advance(
            &pool,
            id,
            merchant_id,
            &provider_slug,
            transition.to,
            transition.event,
            1,
            transition.occurred_at,
            None,
            None,
        )
        .await
        .unwrap();
        payments::schedule_next(&pool, id, Some(now)).await.unwrap();

        let future_clock = SimClock::new(now + Duration::minutes(15), 0.0);
        let mut captured = false;
        for _ in 0..10 {
            let advanced = tick_payments(&pool, &future_clock).await.unwrap();
            if payments::get(&pool, id).await.unwrap().map(|r| r.status)
                == Some(PaymentStatus::Captured)
            {
                captured = true;
                break;
            }
            if advanced == 0 {
                break;
            }
        }

        assert!(captured, "payment never reached captured");
    }
}
