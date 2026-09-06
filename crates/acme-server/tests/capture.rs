//! Proves the inbound capture pipeline end to end against a synthetic
//! router — no provider router exists until M2 (spec §21.14, M1's scope).

use acme_server::capture::layer::{CaptureState, record_exchange};
use acme_server::capture::recorder::{Channel, Recorder};
use acme_server::sim::clock::SimClock;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_test::TestServer;
use chrono::Utc;
use sqlx::SqlitePool;

async fn ok_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "payment_id": "pay_01ARZ3NDEKTSV4RRFFQ69G5FAV" }))
}

async fn boom_handler() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": "boom" })),
    )
}

fn build_app(pool: SqlitePool) -> Router {
    let (recorder, _handle) = Recorder::spawn(pool);
    let clock = SimClock::new(Utc::now(), 60.0);
    let state = CaptureState {
        recorder,
        clock,
        channel: Channel::Api,
    };

    Router::new()
        .route("/ok", get(ok_handler))
        .route("/boom", post(boom_handler))
        .layer(axum::middleware::from_fn_with_state(state, record_exchange))
}

async fn wait_for_exchanges(pool: &SqlitePool, expected: usize) -> Vec<(String, i64, String)> {
    for _ in 0..50 {
        let rows: Vec<(String, i64, String)> = sqlx::query_as(
            "SELECT path, status_code, outcome FROM http_exchanges ORDER BY started_at",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        if rows.len() >= expected {
            return rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {expected} exchanges to be recorded");
}

#[sqlx::test]
async fn records_successful_and_failed_exchanges(pool: SqlitePool) {
    let app = build_app(pool.clone());
    let server = TestServer::new(app).unwrap();

    server.get("/ok").await.assert_status_ok();
    server
        .post("/boom")
        .json(&serde_json::json!({ "card_number": "4111111111111111" }))
        .await
        .assert_status(StatusCode::INTERNAL_SERVER_ERROR);

    let rows = wait_for_exchanges(&pool, 2).await;
    assert_eq!(rows.len(), 2);

    let ok_row = rows.iter().find(|(path, ..)| path == "/ok").unwrap();
    assert_eq!(ok_row.1, 200);
    assert_eq!(ok_row.2, "ok");

    let boom_row = rows.iter().find(|(path, ..)| path == "/boom").unwrap();
    assert_eq!(boom_row.1, 500);
    assert_eq!(boom_row.2, "server_error");
}

#[sqlx::test]
async fn search_key_picks_up_ids_from_the_response_body(pool: SqlitePool) {
    let app = build_app(pool.clone());
    let server = TestServer::new(app).unwrap();

    server.get("/ok").await.assert_status_ok();
    wait_for_exchanges(&pool, 1).await;

    let (search_key,): (Option<String>,) =
        sqlx::query_as("SELECT search_key FROM http_exchanges WHERE path = '/ok'")
            .fetch_one(&pool)
            .await
            .unwrap();

    assert!(
        search_key
            .unwrap()
            .contains("pay_01ARZ3NDEKTSV4RRFFQ69G5FAV")
    );
}

#[sqlx::test]
async fn card_numbers_never_reach_storage_unredacted(pool: SqlitePool) {
    let app = build_app(pool.clone());
    let server = TestServer::new(app).unwrap();

    server
        .post("/boom")
        .json(&serde_json::json!({ "card_number": "4111111111111111" }))
        .await
        .assert_status(StatusCode::INTERNAL_SERVER_ERROR);

    wait_for_exchanges(&pool, 1).await;

    let bodies: Vec<(Vec<u8>,)> = sqlx::query_as("SELECT body FROM http_bodies")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(!bodies.is_empty());
    for (body,) in bodies {
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("4111111111111111"),
            "unredacted PAN leaked into http_bodies"
        );
    }
}
