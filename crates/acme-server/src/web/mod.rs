//! Dashboard HTTP routes: page renders, health check, static assets.

pub mod layout;
pub mod pages;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use tower_http::services::ServeDir;

use crate::state::AppState;

/// Dashboard routes, finalized with `state` and merged with
/// `http::openapi::router` (which builds both provider routers itself, so
/// it needs its own copy of `state`) — the two must merge as same-typed
/// `Router`s, hence resolving each side's state before merging.
pub fn router(state: AppState) -> Router {
    let dashboard = Router::new()
        .route("/", get(overview))
        .route("/healthz", get(healthz))
        .route("/checkout/{token}", get(hosted_checkout_placeholder))
        .nest_service(
            "/static",
            ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/static")),
        )
        .with_state(state.clone());

    dashboard.merge(crate::http::openapi::router(state))
}

async fn overview(State(state): State<AppState>) -> impl IntoResponse {
    let clock_label = state.clock.now().to_rfc3339();
    pages::overview::render(&clock_label)
}

async fn healthz() -> impl IntoResponse {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

/// Acme Pay checkout sessions' `hosted_url` points here (spec §13.7
/// caveat): hosted-page rendering is a dashboard/Maud concern tagged M5,
/// so M2 serves a placeholder rather than a working fake checkout UI.
async fn hosted_checkout_placeholder() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "hosted checkout pages ship in a later milestone",
    )
}
