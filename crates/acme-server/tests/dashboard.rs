//! Dashboard integration tests (spec §17's Dashboard row): every M3 route
//! returns 200 against a real router with real data, and the advance/
//! refund/exception mutations behave — legal moves swap the detail
//! fragment and persist, illegal ones are rejected without writing.

use acme_server::capture::recorder::Recorder;
use acme_server::config::Config;
use acme_server::dashboard::activity;
use acme_server::db::repo::test_support::{
    ACMEPAY_SECRET_KEY, ACMESHIP_SECRET_KEY, seed_reference_merchant,
};
use acme_server::db::repo::{payments, shipments, traffic};
use acme_server::domain::payment::PaymentStatus;
use acme_server::domain::shipment::ShipmentStatus;
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
        activity: activity::Hub::new(),
    };
    TestServer::new(acme_server::web::router(state)).expect("router builds into a test server")
}

fn acmeship_auth(request: axum_test::TestRequest) -> axum_test::TestRequest {
    request.add_header("authorization", format!("Bearer {ACMESHIP_SECRET_KEY}"))
}

fn acmepay_auth(request: axum_test::TestRequest) -> axum_test::TestRequest {
    request.add_header("authorization", format!("Bearer {ACMEPAY_SECRET_KEY}"))
}

async fn create_captured_payment(server: &TestServer) -> String {
    let response = acmepay_auth(server.post("/acmepay/v1/payments"))
        .json(&json!({
            "amount": 10000,
            "currency": "USD",
            "reference": "ORD-DASH-1",
            "payment_method": {
                "type": "card",
                "card": { "number": "4111111111111111", "exp_month": 11, "exp_year": 2029 }
            }
        }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "captured");
    body["id"].as_str().unwrap().to_string()
}

async fn create_shipment(server: &TestServer) -> String {
    let rate_response = acmeship_auth(server.post("/acmeship/v1/rates"))
        .json(&json!({
            "origin": {"postal_code": "28001", "city": "Madrid", "country": "ES"},
            "destination": {"postal_code": "03690", "city": "San Vicent del Raspeig", "country": "ES", "residential": true},
            "packages": [{"weight_grams": 1800, "length_cm": 30, "width_cm": 22, "height_cm": 12}],
            "declared_value": {"amount": 8990, "currency": "EUR"}
        }))
        .await;
    rate_response.assert_status_ok();
    let rate_body: serde_json::Value = rate_response.json();
    let rate_option_id = rate_body["options"][0]["id"].as_str().unwrap().to_string();

    let shipment_response = acmeship_auth(server.post("/acmeship/v1/shipments"))
        .json(&json!({ "rate_option_id": rate_option_id }))
        .await;
    shipment_response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = shipment_response.json();
    body["id"].as_str().unwrap().to_string()
}

#[sqlx::test]
async fn every_m3_route_returns_200_against_seeded_data(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let server = build_app(pool.clone());

    let payment_id = create_captured_payment(&server).await;
    let shipment_id = create_shipment(&server).await;

    // `capture::recorder` writes off an async channel (spec §21.6: never
    // blocks the request), so give the writer task a few ticks to drain.
    let exchanges = {
        let mut found = Vec::new();
        for _ in 0..50 {
            found = traffic::list(
                &pool,
                &traffic::ListFilter {
                    limit: 10,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            if !found.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        found
    };
    assert!(
        !exchanges.is_empty(),
        "creating a payment/shipment must be visible in the traffic inspector"
    );
    let exchange_id = exchanges[0].id;
    let trace_id = exchanges[0].trace_id;

    for path in [
        "/".to_string(),
        "/payments".to_string(),
        format!("/payments/{payment_id}"),
        "/shipments".to_string(),
        format!("/shipments/{shipment_id}"),
        "/traffic".to_string(),
        format!("/traffic/{exchange_id}"),
        format!("/traffic/{exchange_id}/body/request"),
        format!("/traffic/{exchange_id}/body/response"),
        format!("/traffic/traces/{trace_id}"),
        "/partials/payments/rows".to_string(),
        "/partials/shipments/rows".to_string(),
        "/partials/traffic/rows".to_string(),
    ] {
        server.get(&path).await.assert_status_ok();
    }

    let requests_redirect = server.get("/requests?path=foo").await;
    requests_redirect.assert_status(StatusCode::SEE_OTHER);

    let request_detail_redirect = server.get(&format!("/requests/{exchange_id}")).await;
    request_detail_redirect.assert_status(StatusCode::SEE_OTHER);
}

#[sqlx::test]
async fn advance_moves_status_and_rejects_illegal_targets(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let server = build_app(pool.clone());
    let payment_id = create_captured_payment(&server).await;
    let id: acme_server::domain::ids::PaymentId = payment_id.parse().unwrap();

    // Captured -> Disputed is legal.
    let response = server
        .post(&format!("/payments/{payment_id}/advance"))
        .form(&json!({ "to_status": "disputed" }))
        .await;
    response.assert_status_ok();
    assert!(response.text().contains("Disputed"));
    assert_eq!(
        payments::get(&pool, id).await.unwrap().unwrap().status,
        PaymentStatus::Disputed
    );

    // Disputed -> Created is not a legal transition.
    let response = server
        .post(&format!("/payments/{payment_id}/advance"))
        .form(&json!({ "to_status": "created" }))
        .await;
    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        payments::get(&pool, id).await.unwrap().unwrap().status,
        PaymentStatus::Disputed,
        "an illegal transition must not change the stored status"
    );
}

#[sqlx::test]
async fn refund_with_no_amount_refunds_in_full_and_reaches_refunded(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let server = build_app(pool.clone());
    let payment_id = create_captured_payment(&server).await;
    let id: acme_server::domain::ids::PaymentId = payment_id.parse().unwrap();

    let response = server
        .post(&format!("/payments/{payment_id}/refund"))
        .form(&json!({}))
        .await;
    response.assert_status_ok();
    assert!(response.text().contains("Refunded"));

    let row = payments::get(&pool, id).await.unwrap().unwrap();
    assert_eq!(row.status, PaymentStatus::Refunded);
    assert_eq!(row.amount_refunded_cents, row.amount_cents);
    assert_eq!(payments::list_refunds(&pool, id).await.unwrap().len(), 1);
}

#[sqlx::test]
async fn shipment_advance_and_exception_respect_the_state_machine(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let server = build_app(pool.clone());
    let shipment_id = create_shipment(&server).await;
    let id: acme_server::domain::ids::ShipmentId = shipment_id.parse().unwrap();
    assert_eq!(
        shipments::get(&pool, id).await.unwrap().unwrap().status,
        ShipmentStatus::Created
    );

    // Created -> Cancelled is legal and needs no extra data.
    let response = server
        .post(&format!("/shipments/{shipment_id}/advance"))
        .form(&json!({ "to_status": "cancelled" }))
        .await;
    response.assert_status_ok();
    assert_eq!(
        shipments::get(&pool, id).await.unwrap().unwrap().status,
        ShipmentStatus::Cancelled
    );

    // Exception is only legal from `out_for_delivery`; a cancelled
    // shipment must reject it and record no reason.
    let response = server
        .post(&format!("/shipments/{shipment_id}/exception"))
        .form(&json!({ "reason": "lost in transit" }))
        .await;
    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        shipments::get(&pool, id).await.unwrap().unwrap().status,
        ShipmentStatus::Cancelled
    );
}
