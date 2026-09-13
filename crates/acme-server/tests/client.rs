//! Smoke test for `crates/acme-client` (spec `specs/1-Acme-Client.md`):
//! drives one payment and one shipment/rate round trip through the
//! generated `Acme` client instead of raw `reqwest`, against a real
//! (random-port) instance of the app — the generated client makes real
//! HTTP requests over the network stack, unlike `tests/scenarios.rs`'s
//! in-process `TestServer` calls.

use acme_client::Acme;
use acme_server::capture::recorder::Recorder;
use acme_server::config::Config;
use acme_server::dashboard::activity;
use acme_server::db::repo::test_support::{
    ACMEPAY_SECRET_KEY, ACMESHIP_SECRET_KEY, seed_reference_merchant,
};
use acme_server::sim::clock::SimClock;
use acme_server::state::AppState;
use axum_test::{TestServer, TestServerConfig, Transport};
use chrono::Utc;
use serde_json::json;
use sqlx::SqlitePool;

#[sqlx::test]
async fn generated_client_creates_a_payment_and_a_rate_quote(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let clock = SimClock::new(Utc::now(), 0.0);
    let (recorder, _handle) = Recorder::spawn(pool.clone());
    let state = AppState {
        db: pool,
        cfg: Config::default(),
        clock,
        recorder,
        activity: activity::Hub::new(),
        http_client: reqwest::Client::new(),
    };

    let server = TestServer::new_with_config(
        acme_server::web::router(state),
        TestServerConfig {
            transport: Some(Transport::HttpRandomPort),
            ..Default::default()
        },
    )
    .expect("router builds into a real-socket test server");
    let base_url = server
        .server_address()
        .expect("HttpRandomPort transport always has an address");

    let acme = Acme::new(
        base_url.as_str().trim_end_matches('/'),
        ACMEPAY_SECRET_KEY,
        ACMESHIP_SECRET_KEY,
    );

    let payment_request = serde_json::from_value(json!({
        "amount": 4599000,
        "currency": "CLP",
        "payment_method": {
            "type": "card",
            "card": {
                "number": "4111111111111111",
                "exp_month": 11,
                "exp_year": 2029,
                "holder": "CAMILA FUENTES",
                "cvv": "123"
            }
        },
        "reference": "ORD-CLIENT-TEST",
        "metadata": {}
    }))
    .expect("matches CreatePaymentRequest");
    let payment = acme
        .pay
        .create_payment(None, &payment_request)
        .await
        .expect("create_payment succeeds");
    assert!(!payment.id.is_empty());

    let rate_request = serde_json::from_value(json!({
        "origin": {"postal_code": "28001", "city": "Madrid", "country": "ES"},
        "destination": {"postal_code": "03690", "city": "San Vicent del Raspeig", "country": "ES", "residential": true},
        "packages": [{"weight_grams": 1800, "length_cm": 30, "width_cm": 22, "height_cm": 12}],
        "declared_value": {"amount": 8990, "currency": "EUR"}
    }))
    .expect("matches RateRequest");
    let rates = acme
        .ship
        .create_rate(&rate_request)
        .await
        .expect("create_rate succeeds");
    assert!(
        !rates.options.is_empty(),
        "ES route must price at least one service"
    );
}
