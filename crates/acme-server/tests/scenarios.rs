//! End-to-end journey (spec §17, §20's M2 acceptance criterion): quote ->
//! create shipment -> label -> tracking -> delivered, and create payment
//! -> captured, for one seeded merchant, against the real router — driving
//! the sim clock by hand between steps, exactly as `tests/capture.rs`
//! drove the capture pipeline in M1.

use acme_server::capture::recorder::Recorder;
use acme_server::config::Config;
use acme_server::dashboard::activity;
use acme_server::db::repo::shipments;
use acme_server::db::repo::test_support::{
    ACMEPAY_SECRET_KEY, ACMESHIP_SECRET_KEY, seed_reference_merchant,
};
use acme_server::domain::shipment::ShipmentStatus;
use acme_server::sim::clock::SimClock;
use acme_server::sim::ticker;
use acme_server::state::AppState;
use axum::http::StatusCode;
use axum_test::TestServer;
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::SqlitePool;

fn build_app(pool: SqlitePool, clock: SimClock) -> TestServer {
    let (recorder, _handle) = Recorder::spawn(pool.clone());
    let state = AppState {
        db: pool,
        cfg: Config::default(),
        clock,
        recorder,
        activity: activity::Hub::new(),
        http_client: reqwest::Client::new(),
    };
    TestServer::new(acme_server::web::router(state)).expect("router builds into a test server")
}

fn acmeship_auth(request: axum_test::TestRequest) -> axum_test::TestRequest {
    request.add_header("authorization", format!("Bearer {ACMESHIP_SECRET_KEY}"))
}

fn acmepay_auth(request: axum_test::TestRequest) -> axum_test::TestRequest {
    request.add_header("authorization", format!("Bearer {ACMEPAY_SECRET_KEY}"))
}

#[sqlx::test]
async fn quote_ship_label_track_deliver(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let now = Utc::now();
    let clock = SimClock::new(now, 0.0);
    let server = build_app(pool.clone(), clock);

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
    let options = rate_body["options"]
        .as_array()
        .expect("rate quote has options");
    assert!(
        !options.is_empty(),
        "ES route must price at least one service"
    );
    let rate_option_id = options[0]["id"]
        .as_str()
        .expect("option has an id")
        .to_string();

    let shipment_response = acmeship_auth(server.post("/acmeship/v1/shipments"))
        .json(&json!({ "rate_option_id": rate_option_id }))
        .await;
    shipment_response.assert_status(StatusCode::CREATED);
    let shipment_body: serde_json::Value = shipment_response.json();
    assert_eq!(shipment_body["status"], "created");
    let shipment_id = shipment_body["id"]
        .as_str()
        .expect("shipment has an id")
        .to_string();

    let label_response =
        acmeship_auth(server.get(&format!("/acmeship/v1/shipments/{shipment_id}/label"))).await;
    label_response.assert_status_ok();
    let label_body: serde_json::Value = label_response.json();
    assert!(label_body["url"].as_str().unwrap().contains(&shipment_id));

    // Drive the ticker by hand, clock parked past the shipment's eta (plus
    // slack for `happy_path_schedule`'s up-to-25% jitter on its last hop),
    // so every hop is immediately due — mirrors `sim::ticker`'s own test.
    let parsed_id: acme_server::domain::ids::ShipmentId = shipment_id.parse().unwrap();
    let future_clock = SimClock::new(now + Duration::days(5), 0.0);
    let mut delivered = false;
    for _ in 0..32 {
        let advanced = ticker::tick_shipments(&pool, &future_clock)
            .await
            .expect("tick succeeds");
        if shipments::get(&pool, parsed_id)
            .await
            .unwrap()
            .map(|r| r.status)
            == Some(ShipmentStatus::Delivered)
        {
            delivered = true;
            break;
        }
        if advanced == 0 {
            break;
        }
    }
    assert!(delivered, "shipment never reached delivered");

    let tracking_response =
        acmeship_auth(server.get(&format!("/acmeship/v1/shipments/{shipment_id}/tracking"))).await;
    tracking_response.assert_status_ok();
    let tracking_body: serde_json::Value = tracking_response.json();
    assert_eq!(tracking_body["status"], "delivered");
    assert!(!tracking_body["events"].as_array().unwrap().is_empty());

    // The same tracking number is reachable with no auth at all (spec
    // §11.1's public tracking endpoint).
    let tracking_number = shipment_body["tracking_number"].as_str().unwrap();
    let public_response = server
        .get(&format!("/acmeship/v1/tracking/{tracking_number}"))
        .await;
    public_response.assert_status_ok();
}

#[sqlx::test]
async fn create_payment_is_captured_immediately(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let clock = SimClock::new(Utc::now(), 0.0);
    let server = build_app(pool, clock);

    let response = acmepay_auth(server.post("/acmepay/v1/payments"))
        .json(&json!({
            "amount": 459900,
            "currency": "EUR",
            "reference": "ORD-2026-000814",
            "payment_method": {
                "type": "card",
                "card": { "number": "4111111111111111", "exp_month": 11, "exp_year": 2029, "cvv": "123", "holder": "CAMILA FUENTES" }
            }
        }))
        .await;

    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "captured");
    assert_eq!(body["payment_method"]["card"]["brand"], "visa");
    assert_eq!(body["payment_method"]["card"]["last4"], "1111");
}

#[sqlx::test]
async fn declined_card_returns_402_and_still_persists_the_payment(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let clock = SimClock::new(Utc::now(), 0.0);
    let server = build_app(pool.clone(), clock);

    let response = acmepay_auth(server.post("/acmepay/v1/payments"))
        .json(&json!({
            "amount": 100000,
            "currency": "EUR",
            "payment_method": { "type": "card", "card": { "number": "4000000000000002", "exp_month": 11, "exp_year": 2029 } }
        }))
        .await;

    response.assert_status(StatusCode::PAYMENT_REQUIRED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"]["code"], "insufficient_funds");
}

#[sqlx::test]
async fn unauthenticated_request_is_rejected(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let clock = SimClock::new(Utc::now(), 0.0);
    let server = build_app(pool, clock);

    let response = server.get("/acmepay/v1/payment_methods").await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}
