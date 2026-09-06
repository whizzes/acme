//! Acme Pay (`/acmepay/v1`) — the reference payment dialect (spec §10.1).

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

/// Info/tags/security only — paths are collected from `routes!` below, per
/// utoipa-axum's split between "what this API is" and "what routes it has".
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Acme Pay",
        version = "1.0.0",
        description = "Reference payment dialect. Simulated — no real money moves. Click Authorize and paste `sk_test_acmepay_demo` (no real secret — seeded on first boot) to try any operation below.",
        contact(name = "acme sandbox", url = "http://localhost:2263")
    ),
    servers((url = "/acmepay/v1")),
    tags(
        (name = "payments", description = "Create, retrieve, capture, cancel and refund payments"),
        (name = "checkout", description = "Hosted checkout sessions"),
        (name = "events", description = "Merchant event log"),
        (name = "webhook_endpoints", description = "Register callback URLs; matching events are signed and delivered automatically"),
        (name = "payment_methods", description = "Payment methods catalog"),
    ),
    modifiers(&SecurityAddon)
)]
pub struct AcmePayApi;

/// Every route, grouped one `.routes(routes!(...))` call per distinct
/// path — `utoipa-axum`'s macro merges a group into one `MethodRouter` for
/// a single path, so handlers with different paths must NOT share a
/// group, or the second `GET`/`POST`/etc in the group collides with the
/// first (axum panics: "Overlapping method route").
fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(AcmePayApi::openapi())
        .routes(routes!(routes::create_payment, routes::list_payments))
        .routes(routes!(routes::get_payment))
        .routes(routes!(routes::capture_payment))
        .routes(routes!(routes::cancel_payment))
        .routes(routes!(routes::create_refund, routes::list_refunds))
        .routes(routes!(routes::create_checkout_session))
        .routes(routes!(routes::get_checkout_session))
        .routes(routes!(routes::expire_checkout_session))
        .routes(routes!(routes::list_events))
        .routes(routes!(routes::get_event))
        .routes(routes!(routes::create_webhook_endpoint))
        .routes(routes!(
            routes::get_webhook_endpoint,
            routes::delete_webhook_endpoint
        ))
        .routes(routes!(routes::list_payment_methods))
}

/// The `OpenApi` document alone, with no `AppState` (and so no live DB
/// pool or clock) required — `cargo xtask spec-lint` calls this rather
/// than the full `build`, since linting the spec's shape needs none of
/// `build`'s runtime wiring.
pub fn openapi() -> utoipa::openapi::OpenApi {
    router().split_for_parts().1
}

/// Builds the mounted router and its `OpenApi` document. `state` seeds the
/// axum handler state; `capture`/`auth`/`idempotency` are separate
/// middleware states (spec §21's capture pipeline, §10.1's bearer auth,
/// §8.4's idempotency), stacked so capture is outermost (it must see 401s
/// and idempotency conflicts too) and idempotency innermost (it needs the
/// merchant auth resolves).
pub fn build(state: AppState) -> (Router, utoipa::openapi::OpenApi) {
    let (router, api) = router().split_for_parts();

    let capture_state = CaptureState {
        recorder: state.recorder.clone(),
        clock: state.clock.clone(),
        channel: Channel::Api,
    };
    let auth_state = AuthState {
        pool: state.db.clone(),
        provider_slug: "acmepay",
    };
    let idem_state = IdempotencyState {
        pool: state.db.clone(),
        clock: state.clock.clone(),
    };

    let router = router
        .layer(from_fn_with_state(idem_state, idempotency::layer))
        .layer(from_fn_with_state(auth_state, bearer_auth))
        .layer(from_fn_with_state(capture_state, record_exchange))
        .with_state(state);

    (router, api)
}
