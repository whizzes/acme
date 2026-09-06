//! `/shipments` list + `/shipments/{id}` detail (spec §13.3/§13.6).

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use maud::{Markup, html};
use serde::Deserialize;

use crate::db::repo::shipments::{self, ListFilter, ShipmentRow};
use crate::domain::ids::ShipmentId;
use crate::domain::shipment::ShipmentStatus;
use crate::error::AppError;
use crate::providers::shipping::acmeship::map;
use crate::state::AppState;
use crate::web::layout::{Ctx, NavItem, layout};
use crate::web::pages::components;

#[derive(Debug, Deserialize, Default)]
pub struct ShipmentsQuery {
    pub status: Option<String>,
    pub q: Option<String>,
    pub starting_after: Option<String>,
}

fn to_filter(query: &ShipmentsQuery, limit: i64) -> ListFilter {
    ListFilter {
        merchant_id: None,
        provider_slug: None,
        status: query
            .status
            .as_deref()
            .filter(|s| !s.is_empty())
            .and_then(|s| serde_json::from_value(serde_json::Value::String(s.to_string())).ok()),
        search: query.q.clone().filter(|s| !s.is_empty()),
        limit,
        starting_after: query.starting_after.as_deref().and_then(|s| s.parse().ok()),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ShipmentsQuery>,
) -> Result<Markup, AppError> {
    let clock_label = components::timestamp(state.clock.now());
    let rows = shipments::list(&state.db, &to_filter(&query, 50)).await?;

    Ok(layout(
        &Ctx {
            title: "Shipments",
            clock_label: &clock_label,
            active: NavItem::Shipments,
        },
        html! {
            h1 { "Shipments" }
            (filter_form(&query))
            table.data {
                thead { tr {
                    th { "Tracking" } th { "Status" } th { "Route" } th.num { "Price" } th.mono { "Created" }
                } }
                tbody #rows { (row_list(&rows)) }
            }
            @if rows.is_empty() {
                (components::empty_state(
                    "No shipments yet. Create one with the sample request for Acme Ship.",
                    Some("curl -X POST http://localhost:2263/acmeship/v1/shipments \\\n  -H 'Authorization: Bearer sk_test_acmeship_demo' \\\n  -H 'Content-Type: application/json' \\\n  -d '{\"rate_option_id\": \"rto_...\"}'"),
                ))
            }
        },
    ))
}

fn filter_form(query: &ShipmentsQuery) -> Markup {
    html! {
        form.filter-bar hx-get="/partials/shipments/rows" hx-target="#rows" hx-swap="innerHTML"
            hx-trigger="change, keyup delay:300ms from:input[name=q]" hx-push-url="true" hx-indicator="#busy" {
            select name="status" {
                option value="" { "All statuses" }
                @for s in ShipmentStatus::ALL {
                    @let value = components::enum_str(s);
                    option value=(value) selected[query.status.as_deref() == Some(value.as_str())] { (value) }
                }
            }
            input type="search" name="q" placeholder="tracking number…" value=(query.q.clone().unwrap_or_default());
            span #busy.busy-indicator { "loading…" }
        }
    }
}

pub async fn rows_partial(
    State(state): State<AppState>,
    Query(query): Query<ShipmentsQuery>,
) -> Result<Markup, AppError> {
    let rows = shipments::list(&state.db, &to_filter(&query, 50)).await?;
    Ok(row_list(&rows))
}

/// `shipments.origin`/`destination` are stored as opaque JSON text (spec
/// §7.4) in whichever shape the writing dialect chose — Acme Ship's own
/// `map::address_json` writes only `{postal_code, city, country,
/// residential}`, not the full `domain::address::Address` shape, so this
/// reads just the one field every dialect is expected to include.
fn city_of(json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("city").and_then(|c| c.as_str()).map(str::to_string))
        .unwrap_or_else(|| "?".to_string())
}

fn row_list(rows: &[ShipmentRow]) -> Markup {
    html! {
        @for row in rows {
            @let (tone, label) = components::shipment_tone(row.status);
            tr {
                td.mono { a href={ "/shipments/" (row.id) } { (row.tracking_number.clone()) } }
                td { (components::pill(tone, label)) }
                td { (city_of(&row.origin)) " → " (city_of(&row.destination)) }
                td.num { (components::money(row.price_cents, &row.currency)) }
                td.mono { (components::timestamp(row.created_at)) }
            }
        }
    }
}

pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let Ok(id) = id.parse::<ShipmentId>() else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let Some(row) = shipments::get(&state.db, id).await? else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };

    let clock_label = components::timestamp(state.clock.now());
    let fragment = detail_fragment(&state, &row).await?;

    Ok(layout(
        &Ctx {
            title: "Shipment",
            clock_label: &clock_label,
            active: NavItem::Shipments,
        },
        fragment,
    )
    .into_response())
}

fn route_sketch(origin: &str, destination: &str) -> Markup {
    let from = city_of(origin);
    let to = city_of(destination);
    html! {
        svg.route-sketch viewBox="0 0 480 60" {
            line x1="20" y1="30" x2="460" y2="30" {}
            circle cx="20" cy="30" r="5" {}
            circle cx="460" cy="30" r="5" {}
            text x="10" y="50" { (from) }
            text x="410" y="50" { (to) }
        }
    }
}

/// The whole detail body, re-rendered wholesale after a mutation — same
/// pattern as `payments::detail_fragment`.
pub async fn detail_fragment(state: &AppState, row: &ShipmentRow) -> anyhow::Result<Markup> {
    let events = shipments::list_events(&state.db, row.id).await?;

    let dialect = map::shipment_to_dto(row);
    let dialect_json = serde_json::to_string_pretty(&dialect)?;
    let normalized_json = serde_json::to_string_pretty(&serde_json::json!({
        "id": row.id.to_string(),
        "merchant_id": row.merchant_id.to_string(),
        "provider_slug": row.provider_slug,
        "status": row.status,
        "tracking_number": row.tracking_number,
        "price_cents": row.price_cents,
        "currency": row.currency,
        "scenario": row.scenario,
        "created_at": row.created_at.to_rfc3339(),
        "eta_at": row.eta_at.map(|d| d.to_rfc3339()),
    }))?;

    let (tone, label) = components::shipment_tone(row.status);

    Ok(html! {
        div #shipment-detail {
            div.detail-header {
                h1 { (row.tracking_number.clone()) }
                (components::pill(tone, label))
                span.amount { (components::money(row.price_cents, &row.currency)) }
                span { (row.provider_slug.clone()) }
            }

            (route_sketch(&row.origin, &row.destination))

            div.action-bar {
                @for target in row.status.allowed_transitions() {
                    @let target_str = components::enum_str(target);
                    @let vals = format!("{{\"to_status\":\"{target_str}\"}}");
                    button hx-post={ "/shipments/" (row.id) "/advance" } hx-vals=(vals)
                        hx-target="#shipment-detail" hx-swap="outerHTML" {
                        (target_str)
                    }
                }
                form hx-post={ "/shipments/" (row.id) "/exception" } hx-target="#shipment-detail" hx-swap="outerHTML" {
                    input type="text" name="reason" placeholder="exception reason" required;
                    button type="submit" { "Declare exception" }
                }
            }

            h2 { "Tracking timeline" }
            ul.timeline {
                @for event in &events {
                    li { span.ts { (components::timestamp(event.occurred_at)) } (event.description.clone()) }
                }
            }

            h2 { "Label" }
            @if let Some(label_url) = &row.label_url {
                a href=(label_url) target="_blank" { "Open label (" (row.label_format.clone().unwrap_or_default()) ")" }
            } @else {
                p { "No label generated yet." }
            }

            h2 { "Dialect response vs. normalized record" }
            div.compare {
                div { h3 { "Acme Ship response" } (components::json_pretty(&dialect_json)) }
                div { h3 { "Normalized record" } (components::json_pretty(&normalized_json)) }
            }
        }
    })
}
