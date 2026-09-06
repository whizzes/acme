//! Iberex Express (`/iberex`) — SEUR-style last-mile dialect (spec §11.2,
//! specs/006-Dialects.md item 3).

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
use crate::http::auth::{OAuth2State, oauth2_bearer_auth};
use crate::http::openapi::SecurityAddon;
use crate::state::AppState;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Iberex Express",
        version = "1.0.0",
        description = "SEUR-style last-mile dialect. Simulated — no real parcels move. `POST /services/oauth2/token` with the seeded demo username/password/client credentials to get a bearer token, then click Authorize and paste it in to try any other operation below.",
        contact(name = "acme sandbox", url = "http://localhost:2263")
    ),
    servers((url = "/iberex")),
    tags(
        (name = "auth", description = "OAuth2 password grant"),
        (name = "tarifas", description = "Rating"),
        (name = "expediciones", description = "Create, retrieve, track and cancel expeditions"),
    ),
    modifiers(&SecurityAddon)
)]
pub struct IberexApi;

/// Every authenticated route, grouped one `.routes(routes!(...))` call per
/// distinct path — see `providers::payments::acmepay::router`'s twin doc.
fn authed_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(IberexApi::openapi())
        .routes(routes!(routes::create_tarifa))
        .routes(routes!(routes::create_expedicion))
        .routes(routes!(routes::get_expedicion, routes::cancel_expedicion))
        .routes(routes!(routes::seguimiento_expedicion))
}

/// The token exchange itself needs no prior auth — see
/// `providers::shipping::acmeship::public_router`'s twin for why this is a
/// separate `OpenApiRouter` merged before layering rather than a route
/// excluded some other way.
fn public_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(routes::oauth_token))
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

pub fn build(state: AppState) -> (Router, utoipa::openapi::OpenApi) {
    let (authed_router, mut api) = authed_router().split_for_parts();
    let (public_router, public_api) = public_router().split_for_parts();
    api.paths.paths.extend(public_api.paths.paths);

    let capture_state = CaptureState {
        recorder: state.recorder.clone(),
        clock: state.clock.clone(),
        channel: Channel::Api,
    };
    let auth_state = OAuth2State {
        pool: state.db.clone(),
        provider_slug: "iberex",
    };
    let fault_state = crate::sim::fault::FaultState {
        pool: state.db.clone(),
        clock: state.clock.clone(),
        provider_slug: "iberex",
    };

    let authed_router = authed_router
        .layer(from_fn_with_state(auth_state, oauth2_bearer_auth))
        .layer(from_fn_with_state(fault_state.clone(), crate::sim::fault::inject))
        .layer(from_fn_with_state(capture_state.clone(), record_exchange));

    let public_router = public_router
        .layer(from_fn_with_state(fault_state, crate::sim::fault::inject))
        .layer(from_fn_with_state(capture_state, record_exchange));

    let router = Router::new()
        .merge(authed_router)
        .merge(public_router)
        .with_state(state);

    (router, api)
}
