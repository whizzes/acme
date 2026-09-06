//! Dashboard HTTP routes: page renders, health check, static assets.

pub mod layout;
pub mod mutations;
pub mod pages;
pub mod sse;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use tower_http::services::ServeDir;

use crate::error::AppError;
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
        // Payments
        .route("/payments", get(pages::payments::list))
        .route("/payments/{id}", get(pages::payments::detail))
        .route("/payments/{id}/advance", post(mutations::advance_payment))
        .route("/payments/{id}/refund", post(mutations::refund_payment))
        .route(
            "/partials/payments/rows",
            get(pages::payments::rows_partial),
        )
        // Shipments
        .route("/shipments", get(pages::shipments::list))
        .route("/shipments/{id}", get(pages::shipments::detail))
        .route("/shipments/{id}/advance", post(mutations::advance_shipment))
        .route(
            "/shipments/{id}/exception",
            post(mutations::except_shipment),
        )
        .route(
            "/partials/shipments/rows",
            get(pages::shipments::rows_partial),
        )
        // Traffic inspector
        .route("/traffic", get(pages::traffic::list))
        .route("/traffic/{id}", get(pages::traffic::detail))
        .route(
            "/traffic/{id}/body/{which}",
            get(pages::traffic::body_partial),
        )
        .route("/traffic/traces/{trace_id}", get(pages::traffic::trace))
        .route("/partials/traffic/rows", get(pages::traffic::rows_partial))
        // `/requests` is an alias into `/traffic` (spec §13.3)
        .route("/requests", get(requests_alias))
        .route("/requests/{id}", get(request_alias_detail))
        // Live feed
        .route("/events/stream", get(sse::stream))
        .nest_service(
            "/static",
            ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/static")),
        )
        .with_state(state.clone());

    dashboard.merge(crate::http::openapi::router(state))
}

async fn overview(State(state): State<AppState>) -> Result<impl IntoResponse, AppError> {
    Ok(pages::overview::render(&state).await?)
}

async fn healthz() -> impl IntoResponse {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

async fn requests_alias(uri: axum::http::Uri) -> impl IntoResponse {
    let query = uri.query().map(str::to_string).unwrap_or_default();
    let separator = if query.is_empty() { "" } else { "&" };
    Redirect::to(&format!("/traffic?direction=inbound{separator}{query}"))
}

async fn request_alias_detail(Path(id): Path<String>) -> impl IntoResponse {
    Redirect::to(&format!("/traffic/{id}"))
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
