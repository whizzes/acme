//! Acme Pay hosted checkout page integration tests
//! (openspec/changes/acmepay-hosted-checkout): the card form at
//! `/acmepay/c/{cs_id}` resolves the same magic-PAN scenarios the direct
//! API does, closes the checkout session, redirects the shopper, and
//! (via the existing, unmodified dispatcher) delivers a webhook.

use std::sync::Arc;

use acme_server::capture::recorder::Recorder;
use acme_server::config::Config;
use acme_server::dashboard::activity;
use acme_server::dashboard::dispatcher;
use acme_server::db::repo::payments;
use acme_server::db::repo::test_support::{ACMEPAY_SECRET_KEY, seed_reference_merchant};
use acme_server::db::repo::webhooks;
use acme_server::sim::clock::SimClock;
use acme_server::state::AppState;
use axum::extract::State as AxumState;
use axum::http::{StatusCode, header};
use axum::routing::post;
use axum_test::TestServer;
use chrono::Utc;
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

fn build_app(pool: SqlitePool) -> (TestServer, AppState) {
    let (recorder, _handle) = Recorder::spawn(pool.clone());
    let state = AppState {
        db: pool,
        cfg: Config::default(),
        clock: SimClock::new(Utc::now(), 0.0),
        recorder,
        activity: activity::Hub::new(),
        http_client: reqwest::Client::new(),
    };
    let server = TestServer::new(acme_server::web::router(state.clone()))
        .expect("router builds into a test server");
    (server, state)
}

fn acmepay_auth(request: axum_test::TestRequest) -> axum_test::TestRequest {
    request.add_header("authorization", format!("Bearer {ACMEPAY_SECRET_KEY}"))
}

struct Captured {
    body: String,
}

async fn spawn_mock_endpoint(
    status: StatusCode,
) -> (
    String,
    Arc<Mutex<Vec<Captured>>>,
    tokio::task::JoinHandle<()>,
) {
    let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));

    async fn handler(
        AxumState((status, captured)): AxumState<(StatusCode, Arc<Mutex<Vec<Captured>>>)>,
        body: String,
    ) -> StatusCode {
        captured.lock().await.push(Captured { body });
        status
    }

    let app = axum::Router::new()
        .route("/hooks", post(handler))
        .with_state((status, captured.clone()));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    (format!("http://{addr}/hooks"), captured, handle)
}

async fn register_endpoint(server: &TestServer, url: &str) -> String {
    let response = acmepay_auth(server.post("/acmepay/v1/webhook_endpoints"))
        .json(&json!({ "url": url, "enabled_events": ["*"] }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    body["id"].as_str().unwrap().to_string()
}

/// Drains every queued delivery. `webhooks::due` deliberately returns at
/// most one delivery per endpoint per call (spec §12.1: "deliveries to the
/// same endpoint are serialized" — see the query comment in
/// `db/repo/webhooks.rs`), so a payment that fanned out several events
/// (`payment.created`, then `payment.pending`/`payment.authorized`/
/// `payment.captured` from the initial-steps walk) needs several
/// due+dispatch rounds to fully drain, same as `dashboard::dispatcher`'s
/// real 1s-tick loop would do over time.
async fn drain_deliveries(pool: &SqlitePool, state: &AppState) {
    let now = state.clock.now();
    for _ in 0..20 {
        let due = webhooks::due(pool, now, 10).await.unwrap();
        if due.is_empty() {
            return;
        }
        for delivery in &due {
            dispatcher::attempt_delivery(state, delivery, now).await;
        }
    }
    panic!("webhook queue did not drain after 20 dispatcher ticks");
}

/// Creates a checkout session and returns `(id, hosted_path)`, where
/// `hosted_path` is `hosted_url` with the scheme/host stripped so the test
/// can `TestServer::get`/`post` it directly.
async fn create_checkout_session(server: &TestServer) -> (String, String) {
    let response = acmepay_auth(server.post("/acmepay/v1/checkout/sessions"))
        .json(&json!({
            "amount": 10000,
            "currency": "USD",
            "success_url": "https://merchant.example/success",
            "cancel_url": "https://merchant.example/cancel",
        }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    let id = body["id"].as_str().unwrap().to_string();
    let hosted_url = body["hosted_url"].as_str().unwrap().to_string();
    let hosted_path = hosted_url
        .splitn(4, '/')
        .nth(3)
        .map(|rest| format!("/{rest}"))
        .expect("hosted_url has a path component");
    (id, hosted_path)
}

#[sqlx::test]
async fn approved_card_closes_the_session_redirects_and_fires_a_webhook(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let (server, state) = build_app(pool.clone());
    let (url, captured, _handle) = spawn_mock_endpoint(StatusCode::OK).await;
    register_endpoint(&server, &url).await;

    server
        .get("/acmepay/c/does-not-exist")
        .await
        .assert_status_ok();

    let (cs_id, hosted_path) = create_checkout_session(&server).await;

    let form_page = server.get(&hosted_path).await;
    form_page.assert_status_ok();
    assert!(form_page.text().contains("Card number"));

    let response = server
        .post(&hosted_path)
        .form(&json!({
            "number": "4111111111111111",
            "holder": "Camila Fuentes",
            "exp_month": 11,
            "exp_year": 2029,
            "cvv": "123",
        }))
        .await;
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "https://merchant.example/success"
    );

    let session_id = cs_id.parse().expect("valid checkout session id");
    let session = payments::get_checkout_session(&pool, session_id)
        .await
        .unwrap()
        .expect("session still exists");
    assert_eq!(session.status, "closed");
    assert_eq!(session.payment_status, "paid");
    assert!(session.payment_id.is_some());

    drain_deliveries(&pool, &state).await;

    let requests = captured.lock().await;
    assert!(
        requests.iter().any(|r| r.body.contains("payment.captured")),
        "expected a payment.captured delivery, got: {:?}",
        requests.iter().map(|r| &r.body).collect::<Vec<_>>()
    );
}

#[sqlx::test]
async fn declined_card_closes_the_session_unpaid_and_redirects_to_cancel_url(pool: SqlitePool) {
    seed_reference_merchant(&pool).await.expect("seed succeeds");
    let (server, state) = build_app(pool.clone());
    let (url, captured, _handle) = spawn_mock_endpoint(StatusCode::OK).await;
    register_endpoint(&server, &url).await;

    let (cs_id, hosted_path) = create_checkout_session(&server).await;

    let response = server
        .post(&hosted_path)
        .form(&json!({
            "number": "4000000000000002",
            "holder": "Camila Fuentes",
            "exp_month": 11,
            "exp_year": 2029,
            "cvv": "123",
        }))
        .await;
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "https://merchant.example/cancel"
    );

    let session_id = cs_id.parse().expect("valid checkout session id");
    let session = payments::get_checkout_session(&pool, session_id)
        .await
        .unwrap()
        .expect("session still exists");
    assert_eq!(session.status, "closed");
    assert_eq!(session.payment_status, "unpaid");

    drain_deliveries(&pool, &state).await;
    let requests = captured.lock().await;
    assert!(
        requests.iter().any(|r| r.body.contains("payment.rejected")),
        "expected a payment.rejected delivery, got: {:?}",
        requests.iter().map(|r| &r.body).collect::<Vec<_>>()
    );

    // Resubmitting a closed session must not create a second payment.
    let second = server
        .post(&hosted_path)
        .form(&json!({
            "number": "4111111111111111",
            "exp_month": 11,
            "exp_year": 2029,
        }))
        .await;
    second.assert_status_ok();
    assert!(second.text().contains("no longer available"));
    let session_after = payments::get_checkout_session(&pool, session_id)
        .await
        .unwrap()
        .expect("session still exists");
    assert_eq!(
        session_after.payment_status, "unpaid",
        "resubmitting a closed session must not change its outcome"
    );
}
