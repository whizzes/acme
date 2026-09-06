//! Trancorp Webpay (`/rswebpaytransaction/api/webpay/v1.2`) — Transbank-style
//! two-step create→commit dialect (spec §10.2, specs/006-Dialects.md item 2).

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
use crate::http::auth::{HeaderKeyPairState, header_key_pair_auth};
use crate::http::openapi::SecurityAddon;
use crate::state::AppState;

/// Info/tags/security only — paths are collected from `routes!` below, per
/// `providers::payments::acmepay::router`'s twin doc comment.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Trancorp Webpay",
        version = "1.2.0",
        description = "Transbank-style create→commit dialect. Simulated — no real money moves. Click Authorize and paste the seeded `Tbk-Api-Key-Id`/`Tbk-Api-Key-Secret` pair to try any operation below. No card data ever passes through this API — it's entered on the hosted page `POST .../transactions` returns a `url` for.",
        contact(name = "acme sandbox", url = "http://localhost:2263")
    ),
    servers((url = "/rswebpaytransaction/api/webpay/v1.2")),
    tags((name = "transactions", description = "Create, commit, retrieve and refund transactions")),
    modifiers(&SecurityAddon)
)]
pub struct WebpayApi;

/// Every route, grouped one `.routes(routes!(...))` call per distinct
/// path — see `providers::payments::acmepay::router`'s twin doc for why
/// handlers with different paths must not share a group.
fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(WebpayApi::openapi())
        .routes(routes!(routes::create_transaction))
        .routes(routes!(
            routes::commit_transaction,
            routes::get_transaction_status
        ))
        .routes(routes!(routes::refund_transaction))
}

/// The `OpenApi` document alone, with no `AppState` required — see
/// `providers::payments::acmepay::openapi`'s twin for why `xtask
/// spec-lint` wants this rather than the full `build`.
pub fn openapi() -> utoipa::openapi::OpenApi {
    router().split_for_parts().1
}

/// Builds the mounted router and its `OpenApi` document. No idempotency
/// layer — spec §10.6's own comparison table lists Trancorp's idempotency
/// support as `n/a`, unlike the reference dialects.
pub fn build(state: AppState) -> (Router, utoipa::openapi::OpenApi) {
    let (router, api) = router().split_for_parts();

    let capture_state = CaptureState {
        recorder: state.recorder.clone(),
        clock: state.clock.clone(),
        channel: Channel::Api,
    };
    let auth_state = HeaderKeyPairState {
        pool: state.db.clone(),
        provider_slug: "webpay",
        id_header: "tbk-api-key-id",
        secret_header: "tbk-api-key-secret",
    };

    let router = router
        .layer(from_fn_with_state(auth_state, header_key_pair_auth))
        .layer(from_fn_with_state(capture_state, record_exchange))
        .with_state(state);

    (router, api)
}
