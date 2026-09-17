//! Per-provider "Docs" page: renders the project's actual
//! `acme.example.toml` verbatim, plus a one-line purpose for every key in
//! it. Content is the same regardless of provider slug — `acme.toml` is a
//! single global config file, not provider-scoped — but the page still
//! lives under `/providers/{slug}/docs` so every provider's detail page
//! can link to it, and so it can name that specific provider's own
//! webhook-endpoint registration path.

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use maud::html;

use crate::db::repo::providers;
use crate::error::AppError;
use crate::state::AppState;
use crate::web::layout::{Ctx, NavItem, layout};
use crate::web::pages::components;

const ACME_TOML_EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../acme.example.toml"
));

/// `(key, one-line purpose)`, covering every key `ACME_TOML_EXAMPLE`
/// defines. The test below asserts every key here appears in the embedded
/// file, so a renamed or removed key fails loudly instead of silently
/// going stale.
const KEY_DESCRIPTIONS: &[(&str, &str)] = &[
    ("bind", "Address and port the server listens on."),
    ("database_url", "SQLite connection string."),
    (
        "public_url",
        "Base URL used to build hosted checkout links and other absolute URLs the server hands back to clients.",
    ),
    ("seed", "Whether to seed demo data on first boot."),
    ("seed_rng", "RNG seed for deterministic demo data."),
    (
        "seed_scale",
        "How much demo data to seed: small, medium, or large.",
    ),
    (
        "clock_multiplier",
        "How much faster simulated time runs than wall-clock time.",
    ),
    (
        "webhooks_enabled",
        "Global switch for the webhook dispatcher — applies to every provider, including this one.",
    ),
    (
        "webhook_timeout_ms",
        "How long the dispatcher waits for a webhook endpoint to respond before treating the attempt as failed — server-wide, not per provider.",
    ),
    (
        "webhook_max_attempts",
        "How many delivery attempts the dispatcher makes before giving up on a webhook endpoint — server-wide, not per provider.",
    ),
    (
        "request_log_retention",
        "How many inbound request/response rows the traffic inspector keeps.",
    ),
    (
        "request_log_body_limit",
        "Max bytes of a request/response body the traffic inspector stores.",
    ),
    (
        "latency_ms",
        "Artificial latency added to every response, for testing slow-network handling.",
    ),
    (
        "failure_rate",
        "Fraction of requests that fail randomly, for chaos testing.",
    ),
    ("log", "Tracing log filter directive."),
    (
        "dashboard_enabled",
        "Whether the dashboard UI is served at all.",
    ),
    (
        "traffic_capture",
        "How much of each request/response the traffic inspector records: full, headers_only, errors_only, or off.",
    ),
    (
        "traffic_body_limit",
        "Max body bytes captured per request/response.",
    ),
    (
        "traffic_binary_limit",
        "Max bytes captured for a binary body before truncating.",
    ),
    (
        "traffic_retain_rows",
        "Max traffic rows kept before older ones are trimmed.",
    ),
    (
        "traffic_retain_bytes",
        "Max total bytes of captured bodies kept before trimming.",
    ),
    (
        "traffic_retain_hours",
        "Max age of a traffic row before it becomes eligible for trimming.",
    ),
    (
        "traffic_pin_errors",
        "Whether error responses are exempt from traffic retention trimming.",
    ),
];

pub async fn page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let clock_label = components::timestamp(state.clock.now());
    let Some(provider) = providers::get(&state.db, &slug).await? else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let webhook_endpoint_path = format!(
        "{}/webhook_endpoints",
        provider.base_path.trim_end_matches('/')
    );

    Ok(layout(
        &Ctx {
            title: "Provider docs",
            clock_label: &clock_label,
            active: NavItem::Providers,
        },
        html! {
            h1 { (provider.display_name.clone()) " — configuration" }
            p {
                "Every provider's webhook dispatch is configured server-wide in "
                code { "acme.toml" }
                " — there is no per-provider config file. Endpoint URLs and "
                "secrets are registered per merchant through this provider's "
                "own API ("
                code { "POST " (webhook_endpoint_path) }
                ") or the dashboard, not through "
                code { "acme.toml" }
                "."
            }
            h2 { "acme.toml example" }
            pre.curl { (ACME_TOML_EXAMPLE) }
            h2 { "Every key" }
            table.data {
                thead { tr { th { "Key" } th { "Purpose" } } }
                tbody {
                    @for (key, purpose) in KEY_DESCRIPTIONS {
                        tr { td.mono { (key) } td { (purpose) } }
                    }
                }
            }
        },
    )
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_described_key_appears_in_the_embedded_example() {
        for (key, _) in KEY_DESCRIPTIONS {
            assert!(
                ACME_TOML_EXAMPLE.contains(&format!("{key} =")),
                "key `{key}` not found in acme.example.toml — table is stale"
            );
        }
    }
}
