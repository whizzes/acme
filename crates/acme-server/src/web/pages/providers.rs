//! `/providers` catalog + `/providers/{slug}` detail (spec §13.3,
//! specs/006-Dialects.md item 5).

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use maud::{Markup, html};

use crate::db::repo::providers::{self, ProviderRow};
use crate::error::AppError;
use crate::state::AppState;
use crate::web::layout::{Ctx, NavItem, layout};
use crate::web::pages::components;

/// Every provider spec §10/§11 eventually add, shown as a placeholder row
/// until its own module exists — a running checklist for
/// specs/007-Additional-Dialects.md (specs/006-Dialects.md item 5's own
/// reasoning for shipping this page now rather than waiting for all ten).
const PLANNED: &[(&str, &str)] = &[
    ("pagorapido", "Pago Rápido"),
    ("chargeflow", "Chargeflow"),
    ("nordika", "Nordika Pay"),
    ("postalis", "Postalis"),
    ("shiphub", "ShipHub"),
    ("veloz", "Veloz"),
];

pub async fn list(State(state): State<AppState>) -> Result<Markup, AppError> {
    let clock_label = components::timestamp(state.clock.now());
    let rows = providers::list(&state.db).await?;
    let built: std::collections::HashSet<&str> = rows.iter().map(|r| r.slug.as_str()).collect();

    Ok(layout(
        &Ctx {
            title: "Providers",
            clock_label: &clock_label,
            active: NavItem::Providers,
        },
        html! {
            h1 { "Providers" }
            table.data {
                thead { tr { th { "Provider" } th { "Kind" } th { "Dialect" } th { "Auth" } th { "Base path" } th { "" } } }
                tbody {
                    @for row in &rows { (provider_row(row)) }
                    @for (slug, name) in PLANNED {
                        @if !built.contains(slug) { (planned_row(slug, name)) }
                    }
                }
            }
        },
    ))
}

fn provider_row(row: &ProviderRow) -> Markup {
    html! {
        tr {
            td { a href={ "/providers/" (row.slug) } { (row.display_name.clone()) } }
            td { (row.kind.clone()) }
            td { (row.dialect.clone()) }
            td.mono { (row.auth_scheme.clone()) }
            td.mono { (row.base_path.clone()) }
            td { (components::pill(if row.enabled { "ok" } else { "idle" }, if row.enabled { "Built" } else { "Disabled" })) }
        }
    }
}

fn planned_row(slug: &str, name: &str) -> Markup {
    html! {
        tr.provider-row--planned {
            td { (name) }
            td { "—" }
            td { "—" }
            td { "—" }
            td.mono { (slug) }
            td { (components::pill("idle", "Planned")) }
        }
    }
}

pub async fn detail(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let clock_label = components::timestamp(state.clock.now());
    let Some(provider) = providers::get(&state.db, &slug).await? else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let credential = providers::demo_credential(&state.db, &slug).await?;

    Ok(layout(
        &Ctx {
            title: "Provider",
            clock_label: &clock_label,
            active: NavItem::Providers,
        },
        html! {
            h1 { (provider.display_name.clone()) }
            dl.kv {
                dt { "kind" } dd { (provider.kind.clone()) }
                dt { "dialect" } dd { (provider.dialect.clone()) }
                dt { "auth scheme" } dd.mono { (provider.auth_scheme.clone()) }
                dt { "base path" } dd.mono { (provider.base_path.clone()) }
            }

            @if let Some(cred) = &credential {
                h2 { "Sandbox credential" }
                p { "This is a sandbox — credentials are shown in full (spec §18)." }
                dl.kv {
                    @if let Some(public_key) = &cred.public_key {
                        dt { "public key / client id" } dd.mono { (public_key.clone()) }
                    }
                    dt { "secret key" } dd.mono { (cred.secret_key.clone()) }
                }
            }

            h2 { "Try it" }
            a href="/docs" { "Open in Swagger UI" }
            " · "
            a href={ "/providers/" (provider.slug) "/docs" } { "Docs" }
        },
    )
    .into_response())
}
