//! `/traffic` list + detail + trace (spec §21.8) — the traffic inspector,
//! the first reader of `http_exchanges`/`http_bodies`/`exchange_resources`.

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use maud::{Markup, html};
use serde::Deserialize;

use crate::db::repo::traffic::{self, ExchangeRow};
use crate::domain::ids::{ExchangeId, TraceId};
use crate::error::AppError;
use crate::state::AppState;
use crate::web::layout::{Ctx, NavItem, layout};
use crate::web::pages::components;

#[derive(Debug, Deserialize, Default)]
pub struct TrafficQuery {
    pub direction: Option<String>,
    pub provider: Option<String>,
    pub method: Option<String>,
    pub status_class: Option<String>,
    pub path: Option<String>,
    pub replay: Option<String>,
    pub starting_after: Option<String>,
}

fn to_filter(query: &TrafficQuery, limit: i64) -> traffic::ListFilter {
    traffic::ListFilter {
        direction: query.direction.clone().filter(|s| !s.is_empty()),
        provider_slug: query.provider.clone().filter(|s| !s.is_empty()),
        method: query.method.clone().filter(|s| !s.is_empty()),
        status_class: query.status_class.clone().filter(|s| !s.is_empty()),
        path_contains: query.path.clone().filter(|s| !s.is_empty()),
        idempotent_replay_only: query.replay.as_deref() == Some("1"),
        limit,
        starting_after: query.starting_after.as_deref().and_then(|s| s.parse().ok()),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<TrafficQuery>,
) -> Result<Markup, AppError> {
    let clock_label = components::timestamp(state.clock.now());
    let rows = traffic::list(&state.db, &to_filter(&query, 100)).await?;

    Ok(layout(
        &Ctx {
            title: "Traffic",
            clock_label: &clock_label,
            active: NavItem::Traffic,
        },
        html! {
            h1 { "Traffic" }
            (filter_form(&query))
            table.data {
                thead { tr {
                    th.mono { "time" } th { "dir" } th { "provider" } th { "method" } th { "path" } th.num { "status" } th.num { "took" }
                } }
                tbody #rows { (row_list(&rows)) }
            }
            @if rows.is_empty() {
                (components::empty_state("No traffic yet. Call any provider endpoint and it will appear here within a second.", None))
            }
        },
    ))
}

fn filter_form(query: &TrafficQuery) -> Markup {
    html! {
        form.filter-bar hx-get="/partials/traffic/rows" hx-target="#rows" hx-swap="innerHTML"
            hx-trigger="change, keyup delay:300ms from:input[name=path]" hx-push-url="true" hx-indicator="#busy" {
            select name="direction" {
                option value="" { "all directions" }
                option value="inbound" selected[query.direction.as_deref() == Some("inbound")] { "inbound" }
                option value="outbound" selected[query.direction.as_deref() == Some("outbound")] { "outbound" }
            }
            select name="method" {
                option value="" { "all methods" }
                @for m in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
                    option value=(m) selected[query.method.as_deref() == Some(m)] { (m) }
                }
            }
            select name="status_class" {
                option value="" { "all statuses" }
                @for c in ["2", "3", "4", "5"] {
                    option value=(c) selected[query.status_class.as_deref() == Some(c)] { (c) "xx" }
                }
            }
            input type="search" name="path" placeholder="path contains…" value=(query.path.clone().unwrap_or_default());
            span #busy.busy-indicator { "loading…" }
        }
    }
}

pub async fn rows_partial(
    State(state): State<AppState>,
    Query(query): Query<TrafficQuery>,
) -> Result<Markup, AppError> {
    let rows = traffic::list(&state.db, &to_filter(&query, 100)).await?;
    Ok(row_list(&rows))
}

fn row_tone(row: &ExchangeRow) -> &'static str {
    match row.status_code {
        Some(code) if code >= 500 => "bad",
        Some(code) if code >= 400 => "warn",
        _ if row.outcome != "ok" => "bad",
        _ => "ok",
    }
}

fn row_list(rows: &[ExchangeRow]) -> Markup {
    html! {
        @for row in rows {
            @let class = format!("exchange-row--{}", row_tone(row));
            tr class=(class) {
                td.mono { a href={ "/traffic/" (row.id) } { (components::timestamp(row.started_at)) } }
                td { (row.direction.clone()) }
                td { (row.provider_slug.clone().unwrap_or_default()) }
                td.mono { (row.method.clone()) }
                td.mono { (row.path.clone()) }
                td.num { @if let Some(code) = row.status_code { (code) } @else { "—" } }
                td.num { @if let Some(ms) = row.duration_ms { (ms) "ms" } @else { "—" } }
            }
        }
    }
}

pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let Ok(id) = id.parse::<ExchangeId>() else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let Some(detail) = traffic::get(&state.db, id).await? else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let resources = traffic::resources_for(&state.db, id).await?;
    let clock_label = components::timestamp(state.clock.now());

    Ok(layout(
        &Ctx {
            title: "Exchange",
            clock_label: &clock_label,
            active: NavItem::Traffic,
        },
        html! {
            h1 { (detail.exchange.method.clone()) " " (detail.exchange.path.clone()) }

            div.traffic-panes {
                div {
                    h3 { "Summary" }
                    dl.kv {
                        dt { "trace" } dd { a href={ "/traffic/traces/" (detail.exchange.trace_id) } { (detail.exchange.trace_id) } }
                        dt { "direction" } dd { (detail.exchange.direction.clone()) }
                        dt { "channel" } dd { (detail.exchange.channel.clone()) }
                        dt { "url" } dd { (detail.url.clone()) }
                        dt { "status" } dd { @if let Some(code) = detail.exchange.status_code { (code) } @else { "—" } }
                        dt { "outcome" } dd { (detail.exchange.outcome.clone()) }
                        dt { "started (sim)" } dd { (components::timestamp(detail.exchange.sim_at)) }
                        dt { "duration" } dd { @if let Some(ms) = detail.exchange.duration_ms { (ms) "ms" } @else { "—" } }
                        dt { "idempotency key" } dd {
                            @if let Some(key) = &detail.exchange.idempotency_key { (key) @if detail.exchange.idempotent_replay { " (replay)" } }
                            @else { "—" }
                        }
                        @if let Some(err) = &detail.transport_error { dt { "transport error" } dd { (err.clone()) } }
                        @if let Some(err) = &detail.error_code { dt { "error code" } dd { (err.clone()) } }
                    }
                }

                div {
                    h3 { "Request" }
                    div hx-get={ "/traffic/" (id) "/body/request" } hx-trigger="revealed" hx-swap="innerHTML" {
                        "loading…"
                    }
                }

                div {
                    h3 { "Response" }
                    div hx-get={ "/traffic/" (id) "/body/response" } hx-trigger="revealed" hx-swap="innerHTML" {
                        "loading…"
                    }
                }

                div {
                    h3 { "Related" }
                    @if resources.is_empty() {
                        p { "No resources touched by this exchange." }
                    } @else {
                        table.data {
                            thead { tr { th { "type" } th { "id" } th { "role" } } }
                            tbody {
                                @for resource in &resources {
                                    tr {
                                        td { (resource.resource_type.clone()) }
                                        td.mono { (resource_link(resource)) }
                                        td { (resource.role.clone()) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
    .into_response())
}

fn resource_link(resource: &traffic::ResourceRef) -> Markup {
    let href = match resource.resource_type.as_str() {
        "payment" => Some(format!("/payments/{}", resource.resource_id)),
        "shipment" => Some(format!("/shipments/{}", resource.resource_id)),
        _ => None,
    };
    match href {
        Some(href) => html! { a href=(href) { (resource.resource_id.clone()) } },
        None => html! { (resource.resource_id.clone()) },
    }
}

#[derive(Deserialize)]
pub struct BodyPath {
    pub id: String,
    pub which: String,
}

/// Progressive disclosure (spec §13.4 pattern 4): bodies are never sent
/// until the pane scrolls into view, so the list/detail pages stay fast
/// even with a large `http_bodies` table.
pub async fn body_partial(
    State(state): State<AppState>,
    Path(BodyPath { id, which }): Path<BodyPath>,
) -> Result<Markup, AppError> {
    let Ok(id) = id.parse::<ExchangeId>() else {
        return Ok(html! { p { "not found" } });
    };
    let Some(detail) = traffic::get(&state.db, id).await? else {
        return Ok(html! { p { "not found" } });
    };

    let (headers_json, body, bytes) = match which.as_str() {
        "response" => (
            detail.response_headers,
            detail.response_body,
            detail.response_bytes,
        ),
        _ => (
            Some(detail.request_headers),
            detail.request_body,
            detail.request_bytes,
        ),
    };

    Ok(html! {
        @if let Some(headers_json) = &headers_json {
            h3 { "Headers" }
            (render_headers(headers_json))
        }
        h3 { "Body (" (bytes) " bytes)" }
        @match body {
            Some(body) => {
                @if body.truncated { p { "Body truncated at the configured capture cap." } }
                @match body.body.as_deref().and_then(|b| std::str::from_utf8(b).ok()) {
                    Some(text) => (components::json_pretty(text)),
                    None => p { "binary or non-utf8 body, " (body.size_bytes) " bytes" },
                }
            }
            None => p { "no body" },
        }
    })
}

fn render_headers(raw: &str) -> Markup {
    let pairs: Vec<(String, String)> = serde_json::from_str(raw).unwrap_or_default();
    html! {
        dl.kv {
            @for (k, v) in &pairs {
                dt { (k.clone()) } dd { (v.clone()) }
            }
        }
    }
}

pub async fn trace(
    State(state): State<AppState>,
    Path(trace_id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let Ok(trace_id) = trace_id.parse::<TraceId>() else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let chain = traffic::trace(&state.db, trace_id).await?;
    let clock_label = components::timestamp(state.clock.now());

    Ok(layout(
        &Ctx {
            title: "Trace",
            clock_label: &clock_label,
            active: NavItem::Traffic,
        },
        html! {
            h1 { "Trace " (trace_id) }
            table.data {
                thead { tr { th.mono { "time" } th { "dir" } th.mono { "method" } th.mono { "path" } th.num { "status" } th.num { "took" } } }
                tbody { (row_list(&chain)) }
            }
        },
    )
    .into_response())
}
