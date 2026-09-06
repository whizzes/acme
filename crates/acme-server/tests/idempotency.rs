//! Idempotency (spec §8.4, §17): two concurrent submissions under the same
//! `Acme-Idempotency-Key` must produce exactly one payment and byte-identical
//! response bodies; a reused key with a different body must be rejected.

use acme_server::capture::recorder::Recorder;
use acme_server::config::Config;
use acme_server::db::repo::test_support::{ACMEPAY_SECRET_KEY, seed_reference_merchant};
use acme_server::sim::clock::SimClock;
use acme_server::state::AppState;
use axum::http::StatusCode;
use axum_test::TestServer;
use chrono::Utc;
use serde_json::json;
use sqlx::SqlitePool;

fn build_app(pool: SqlitePool) -> TestServer {
    let (recorder, _handle) = Recorder::spawn(pool.clone());
    let state = AppState {
        db: pool,
        cfg: Config::default(),
        clock: SimClock::new(Utc::now(), 0.0),
        recorder,
    };
    TestServer::new(acme_server::web::router(state)).expect("router builds into a test server")
}

fn payment_body() -> serde_json::Value {
    json!({
        "amount": 10_000,
        "currency": "USD",
        "payment_method": { "type": "card", "card": { "number": "4111111111111111", "exp_month": 11, "exp_year": 2029 } }
    })
}

#[sqlx::test]
async fn concurrent_duplicate_submission_creates_exactly_one_payment(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let server = build_app(pool.clone());
    let key = "9f3a1c0b-1a4a-4e2f-9f7b-0f2c1c0b9a10";

    let request = || {
        server
            .post("/acmepay/v1/payments")
            .add_header("authorization", format!("Bearer {ACMEPAY_SECRET_KEY}"))
            .add_header("acme-idempotency-key", key)
            .json(&payment_body())
    };

    let (first, second) = tokio::join!(request(), request());

    // Exactly one of the two wins the race and gets `201`. The loser sees
    // either the winner's completed response (`201`, replayed byte-for-
    // byte) or, if it's still mid-flight, spec §8.4 rule 6's `409` retry
    // hint — both are correct outcomes of a genuine race, never a second
    // payment.
    let statuses = [first.status_code(), second.status_code()];
    assert!(
        statuses.contains(&StatusCode::CREATED),
        "at least one request must succeed: {statuses:?}"
    );
    for status in statuses {
        assert!(
            status == StatusCode::CREATED || status == StatusCode::CONFLICT,
            "unexpected status {status}, expected 201 or 409"
        );
    }
    if first.status_code() == StatusCode::CREATED && second.status_code() == StatusCode::CREATED {
        assert_eq!(
            first.text(),
            second.text(),
            "replay must be byte-identical to the original response"
        );
    }

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM payments")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "exactly one payment must have been created");
}

#[sqlx::test]
async fn reused_key_with_a_different_body_is_rejected(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let server = build_app(pool.clone());
    let key = "reused-key-different-body";

    let first = server
        .post("/acmepay/v1/payments")
        .add_header("authorization", format!("Bearer {ACMEPAY_SECRET_KEY}"))
        .add_header("acme-idempotency-key", key)
        .json(&payment_body())
        .await;
    first.assert_status(StatusCode::CREATED);

    let mut different = payment_body();
    different["amount"] = json!(20_000);
    let second = server
        .post("/acmepay/v1/payments")
        .add_header("authorization", format!("Bearer {ACMEPAY_SECRET_KEY}"))
        .add_header("acme-idempotency-key", key)
        .json(&different)
        .await;

    second.assert_status(StatusCode::CONFLICT);

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM payments")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "the second, conflicting request must not have created a payment"
    );
}

#[sqlx::test]
async fn replay_is_marked_with_the_idempotent_replay_header(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let server = build_app(pool.clone());
    let key = "replay-header-key";

    let first = server
        .post("/acmepay/v1/payments")
        .add_header("authorization", format!("Bearer {ACMEPAY_SECRET_KEY}"))
        .add_header("acme-idempotency-key", key)
        .json(&payment_body())
        .await;
    first.assert_status(StatusCode::CREATED);

    let second = server
        .post("/acmepay/v1/payments")
        .add_header("authorization", format!("Bearer {ACMEPAY_SECRET_KEY}"))
        .add_header("acme-idempotency-key", key)
        .json(&payment_body())
        .await;

    second.assert_status(StatusCode::CREATED);
    assert_eq!(second.header("acme-idempotent-replay"), "true");
}
