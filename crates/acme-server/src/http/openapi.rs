//! OpenAPI/Swagger UI assembly (spec §14): one document per provider plus
//! a combined one, all served from a single Swagger UI with a spec
//! selector, with `persistAuthorization` so a bearer token survives a
//! page reload.

use axum::Router;
use utoipa::Modify;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa_swagger_ui::{Config, SwaggerUi, Url};

use crate::providers::payments::acmepay;
use crate::providers::shipping::acmeship;
use crate::state::AppState;

/// Registers the `bearer_token` security scheme both reference dialects
/// declare on every operation (spec §10.1's `Authorization: Bearer
/// sk_test_…`).
pub struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer_token",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("token")
                    .build(),
            ),
        );
    }
}

/// Builds both provider routers, mounts them under their dialect paths,
/// serves each spec's raw JSON, a combined spec, and Swagger UI at
/// `/docs` (spec §14's "additional documentation surfaces").
pub fn router(state: AppState) -> Router {
    let (acmepay_router, acmepay_api) = acmepay::build(state.clone());
    let (acmeship_router, acmeship_api) = acmeship::build(state.clone());

    let mut combined = acmepay_api.clone();
    combined.merge(acmeship_api.clone());

    Router::new()
        .nest("/acmepay/v1", acmepay_router)
        .nest("/acmeship/v1", acmeship_router)
        .merge(
            SwaggerUi::new("/docs")
                .config(Config::default().persist_authorization(true))
                .urls(vec![
                    (Url::new("All providers", "/openapi/acme.json"), combined),
                    (Url::new("Acme Pay", "/openapi/acmepay.json"), acmepay_api),
                    (
                        Url::new("Acme Ship", "/openapi/acmeship.json"),
                        acmeship_api,
                    ),
                ]),
        )
}
