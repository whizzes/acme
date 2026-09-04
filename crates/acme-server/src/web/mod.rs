//! Dashboard HTTP routes: page renders, health check, static assets.

pub mod layout;
pub mod pages;

use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use tower_http::services::ServeDir;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(overview))
        .route("/healthz", get(healthz))
        .nest_service(
            "/static",
            ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/static")),
        )
        .with_state(state)
}

async fn overview(State(state): State<AppState>) -> impl IntoResponse {
    let clock_label = state.clock.now().to_rfc3339();
    pages::overview::render(&clock_label)
}

async fn healthz() -> impl IntoResponse {
    axum::Json(serde_json::json!({ "status": "ok" }))
}
