//! Acme Ship (`/acmeship/v1`) — the reference last-mile dialect (spec §11.1).

pub mod dto;
pub mod map;
pub mod routes;

use axum::Router;
use axum::middleware::from_fn_with_state;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::capture::layer::{CaptureState, record_exchange};
use crate::capture::recorder::Channel;
use crate::http::auth::{AuthState, bearer_auth};
use crate::http::idempotency::{self, IdempotencyState};
use crate::http::openapi::SecurityAddon;
use crate::state::AppState;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Acme Ship",
        version = "1.0.0",
        description = "Reference last-mile dialect. Simulated — no real parcels move. Click Authorize and paste `sk_test_acmeship_demo` (no real secret — seeded on first boot) to try any operation below.",
        contact(name = "acme sandbox", url = "http://localhost:2263")
    ),
    servers((url = "/acmeship/v1")),
    tags(
        (name = "rates", description = "Shipping rate quotes"),
        (name = "shipments", description = "Create, retrieve, label and track shipments"),
        (name = "tracking", description = "Public tracking, no authentication required"),
        (name = "pickups", description = "Collection requests"),
        (name = "returns", description = "Return labels for existing shipments"),
        (name = "coverage", description = "Serviceability and the service catalog"),
    ),
    modifiers(&SecurityAddon)
)]
pub struct AcmeShipApi;

/// Every authenticated route, grouped one `.routes(routes!(...))` call per
/// distinct path — see `providers::payments::acmepay::router`'s twin
/// doc for why handlers with different paths must not share a group.
fn authed_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(AcmeShipApi::openapi())
        .routes(routes!(routes::create_rate))
        .routes(routes!(routes::create_shipment, routes::list_shipments))
        .routes(routes!(routes::get_shipment))
        .routes(routes!(routes::get_shipment_label))
        .routes(routes!(routes::get_shipment_tracking))
        .routes(routes!(routes::cancel_shipment))
        .routes(routes!(routes::create_pickup))
        .routes(routes!(routes::get_pickup))
        .routes(routes!(routes::create_return))
        .routes(routes!(routes::get_coverage))
        .routes(routes!(routes::list_services))
}

fn public_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(routes::public_tracking))
}

/// The `OpenApi` document alone, with no `AppState` required — see
/// `providers::payments::acmepay::openapi`'s twin for why `xtask
/// spec-lint` wants this rather than the full `build`.
pub fn openapi() -> utoipa::openapi::OpenApi {
    let (_, mut api) = authed_router().split_for_parts();
    let (_, public_api) = public_router().split_for_parts();
    api.paths.paths.extend(public_api.paths.paths);
    api
}

/// `public_tracking` (spec §11.1: "Public tracking, no auth") is built as a
/// separate `OpenApiRouter` so it can skip the `bearer_auth` layer the rest
/// of this dialect's routes carry — the two are merged, both as routers
/// and as `OpenApi` documents, before nesting under `/acmeship/v1`.
pub fn build(state: AppState) -> (Router, utoipa::openapi::OpenApi) {
    let (authed_router, mut api) = authed_router().split_for_parts();
    let (public_router, public_api) = public_router().split_for_parts();
    api.paths.paths.extend(public_api.paths.paths);

    let capture_state = CaptureState {
        recorder: state.recorder.clone(),
        clock: state.clock.clone(),
        channel: Channel::Api,
    };
    let auth_state = AuthState {
        pool: state.db.clone(),
        provider_slug: "acmeship",
    };
    let idem_state = IdempotencyState {
        pool: state.db.clone(),
        clock: state.clock.clone(),
    };

    let authed_router = authed_router
        .layer(from_fn_with_state(idem_state, idempotency::layer))
        .layer(from_fn_with_state(auth_state, bearer_auth))
        .layer(from_fn_with_state(capture_state.clone(), record_exchange));

    let public_router = public_router.layer(from_fn_with_state(capture_state, record_exchange));

    let router = Router::new()
        .merge(authed_router)
        .merge(public_router)
        .with_state(state);

    (router, api)
}
