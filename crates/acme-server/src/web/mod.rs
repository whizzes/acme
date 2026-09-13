//! Dashboard HTTP routes: page renders, health check, static assets.

pub mod layout;
pub mod mutations;
pub mod pages;
pub mod sse;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use rust_embed::RustEmbed;

use crate::error::AppError;
use crate::state::AppState;

/// `app.css`/`app.js`/htmx et al. `rust_embed` reads `static/` off disk at
/// runtime in debug builds (`cargo run`/`just run` keep working exactly
/// like the old `tower_http::services::ServeDir` did), but bakes the file
/// bytes into the binary at compile time in release builds — the profile
/// `just build-release`/`docker/docker-build.sh` ship. That's the fix:
/// the old `ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/static"))`
/// baked in a *path*, valid only on the machine that compiled it, and
/// `docker/Dockerfile` copies just the `acme` binary — so that path never
/// existed in the container and every `/static/*` request 404'd. Same
/// fix `sqlx::migrate!("./migrations")` already relies on for
/// migrations: bake the bytes in, not a path to them.
#[derive(RustEmbed)]
#[folder = "static/"]
struct StaticAssets;

async fn serve_static(Path(path): Path<String>) -> impl IntoResponse {
    match StaticAssets::get(&path) {
        Some(file) => (
            [(header::CONTENT_TYPE, file.metadata.mimetype())],
            file.data,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Dashboard routes, finalized with `state` and merged with
/// `http::openapi::router` (which builds both provider routers itself, so
/// it needs its own copy of `state`) — the two must merge as same-typed
/// `Router`s, hence resolving each side's state before merging.
pub fn router(state: AppState) -> Router {
    let dashboard = Router::new()
        .route("/", get(overview))
        .route("/healthz", get(healthz))
        .route("/checkout/{token}", get(hosted_checkout_placeholder))
        // Providers catalog
        .route("/providers", get(pages::providers::list))
        .route("/providers/{slug}", get(pages::providers::detail))
        // Simulator (spec §13.6)
        .route("/simulator", get(pages::simulator::page))
        .route("/sim/clock", post(mutations::set_sim_clock))
        .route("/sim/settings", post(mutations::set_sim_settings))
        .route("/sim/faults", post(mutations::create_sim_fault))
        .route(
            "/sim/faults/{id}",
            axum::routing::delete(mutations::delete_sim_fault),
        )
        .route("/sim/reset", post(mutations::reset_sim))
        // Public tracking pages (spec §13.3), no dashboard chrome
        .route("/t/{provider}/{tracking}", get(pages::tracking::page))
        // Trancorp Webpay's hosted checkout page
        .route(
            "/webpay/checkout",
            get(pages::checkout::page).post(pages::checkout::submit),
        )
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
        // Webhooks
        .route("/webhooks", get(pages::webhooks::list))
        .route(
            "/webhooks/endpoints/{id}",
            get(pages::webhooks::endpoint_detail),
        )
        .route(
            "/webhooks/endpoints/{id}/toggle",
            post(pages::webhooks::toggle_row),
        )
        .route(
            "/webhooks/deliveries/{id}",
            get(pages::webhooks::delivery_detail),
        )
        .route(
            "/webhooks/deliveries/{id}/retry",
            post(mutations::retry_delivery),
        )
        .route(
            "/webhooks/deliveries/{id}/resend",
            post(mutations::resend_delivery),
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
        .route("/static/{*path}", get(serve_static))
        .with_state(state.clone());

    let admin = crate::http::admin::router().with_state(state.clone());

    dashboard
        .merge(admin)
        .merge(crate::http::openapi::router(state))
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
/// caveat). specs/006-Dialects.md item 4 built Trancorp Webpay's own
/// hosted page (`/webpay/checkout`, above) as this milestone's one
/// example; Acme Pay's own page and the other dialects' are
/// specs/007-Additional-Dialects.md's job.
async fn hosted_checkout_placeholder() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "hosted checkout pages ship in a later milestone",
    )
}
