//! `/payments` list + `/payments/{id}` detail (spec §13.3/§13.6).

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use maud::{Markup, html};
use serde::Deserialize;

use crate::db::repo::payments::{self, ListFilter, PaymentRow};
use crate::domain::ids::PaymentId;
use crate::domain::payment::{PaymentStatus, PaymentStatus::*};
use crate::error::AppError;
use crate::providers::payments::acmepay::map;
use crate::state::AppState;
use crate::web::layout::{Ctx, NavItem, layout};
use crate::web::pages::components;

#[derive(Debug, Deserialize, Default)]
pub struct PaymentsQuery {
    pub status: Option<String>,
    pub q: Option<String>,
    pub starting_after: Option<String>,
}

fn to_filter(query: &PaymentsQuery, limit: i64) -> ListFilter {
    ListFilter {
        merchant_id: None,
        provider_slug: None,
        status: query
            .status
            .as_deref()
            .filter(|s| !s.is_empty())
            .and_then(|s| serde_json::from_value(serde_json::Value::String(s.to_string())).ok()),
        created_after: None,
        search: query.q.clone().filter(|s| !s.is_empty()),
        limit,
        starting_after: query.starting_after.as_deref().and_then(|s| s.parse().ok()),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<PaymentsQuery>,
) -> Result<Markup, AppError> {
    let clock_label = components::timestamp(state.clock.now());
    let rows = payments::list(&state.db, &to_filter(&query, 50)).await?;

    Ok(layout(
        &Ctx {
            title: "Payments",
            clock_label: &clock_label,
            active: NavItem::Payments,
        },
        html! {
            h1 { "Payments" }
            (filter_form(&query))
            table.data {
                thead { tr {
                    th { "ID" } th { "Status" } th.num { "Amount" } th { "Reference" } th.mono { "Created" }
                } }
                tbody #rows { (row_list(&rows)) }
            }
            @if rows.is_empty() {
                (components::empty_state(
                    "No payments yet.",
                    Some(("/providers/acmepay", "Create one on the Acme Pay provider page")),
                ))
            }
        },
    ))
}

fn filter_form(query: &PaymentsQuery) -> Markup {
    html! {
        form.filter-bar hx-get="/partials/payments/rows" hx-target="#rows" hx-swap="innerHTML"
            hx-trigger="change, keyup delay:300ms from:input[name=q]" hx-push-url="true" hx-indicator="#busy" {
            select name="status" {
                option value="" { "All statuses" }
                @for s in PaymentStatus::ALL {
                    @let value = components::enum_str(s);
                    option value=(value) selected[query.status.as_deref() == Some(value.as_str())] { (value) }
                }
            }
            input type="search" name="q" placeholder="reference…" value=(query.q.clone().unwrap_or_default());
            span #busy.busy-indicator { "loading…" }
        }
    }
}

pub async fn rows_partial(
    State(state): State<AppState>,
    Query(query): Query<PaymentsQuery>,
) -> Result<Markup, AppError> {
    let rows = payments::list(&state.db, &to_filter(&query, 50)).await?;
    Ok(row_list(&rows))
}

fn row_list(rows: &[PaymentRow]) -> Markup {
    html! {
        @for row in rows {
            @let (tone, label) = components::payment_tone(row.status);
            tr {
                td.mono { a href={ "/payments/" (row.id) } { (row.id) } }
                td { (components::pill(tone, label)) }
                td.num { (components::money(row.amount_cents, &row.currency)) }
                td { (row.reference.clone().unwrap_or_default()) }
                td.mono { (components::timestamp(row.created_at)) }
            }
        }
    }
}

pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let Ok(id) = id.parse::<PaymentId>() else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let Some(row) = payments::get(&state.db, id).await? else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };

    let clock_label = components::timestamp(state.clock.now());
    let fragment = detail_fragment(&state, &row).await?;

    Ok(layout(
        &Ctx {
            title: "Payment",
            clock_label: &clock_label,
            active: NavItem::Payments,
        },
        fragment,
    )
    .into_response())
}

/// The whole detail body, re-rendered wholesale after a mutation (spec
/// §13.4 pattern 3, scaled from "swap one row" to "swap the whole
/// detail" — a resource detail page has no row of its own).
pub async fn detail_fragment(state: &AppState, row: &PaymentRow) -> anyhow::Result<Markup> {
    let events = payments::list_events(&state.db, row.id).await?;
    let refunds = payments::list_refunds(&state.db, row.id).await?;

    let dialect = map::payment_to_dto(row);
    let dialect_json = serde_json::to_string_pretty(&dialect)?;
    let normalized_json = serde_json::to_string_pretty(&serde_json::json!({
        "id": row.id.to_string(),
        "merchant_id": row.merchant_id.to_string(),
        "provider_slug": row.provider_slug,
        "status": row.status,
        "amount_cents": row.amount_cents,
        "amount_refunded_cents": row.amount_refunded_cents,
        "currency": row.currency,
        "scenario": row.scenario,
        "created_at": row.created_at.to_rfc3339(),
        "captured_at": row.captured_at.map(|d| d.to_rfc3339()),
    }))?;

    let (tone, label) = components::payment_tone(row.status);
    let refundable = matches!(row.status, Captured | PartiallyRefunded);

    Ok(html! {
        div #payment-detail {
            div.detail-header {
                h1 { (row.id) }
                (components::pill(tone, label))
                span.amount { (components::money(row.amount_cents, &row.currency)) }
                span { (row.provider_slug.clone()) }
                @if let Some(reference) = &row.reference { span { "ref " (reference) } }
            }

            div.action-bar {
                @for target in row.status.allowed_transitions() {
                    @let target_str = components::enum_str(target);
                    @let vals = format!("{{\"to_status\":\"{target_str}\"}}");
                    button hx-post={ "/payments/" (row.id) "/advance" } hx-vals=(vals)
                        hx-target="#payment-detail" hx-swap="outerHTML" {
                        (target_str)
                    }
                }
                @if refundable {
                    form hx-post={ "/payments/" (row.id) "/refund" } hx-target="#payment-detail" hx-swap="outerHTML" {
                        input type="number" name="amount_cents" placeholder="cents (blank = full remaining)";
                        button type="submit" { "Refund" }
                    }
                }
            }

            h2 { "Timeline" }
            ul.timeline {
                @for event in &events {
                    li { span.ts { (components::timestamp(event.occurred_at)) } (event.kind) " · " (event.source) }
                }
            }

            @if !refunds.is_empty() {
                h2 { "Refunds" }
                table.data {
                    thead { tr { th.mono { "ID" } th.num { "Amount" } th { "Reason" } th.mono { "Created" } } }
                    tbody {
                        @for refund in &refunds {
                            tr {
                                td.mono { (refund.id) }
                                td.num { (components::money(refund.amount_cents, &row.currency)) }
                                td { (refund.reason.clone().unwrap_or_default()) }
                                td.mono { (components::timestamp(refund.created_at)) }
                            }
                        }
                    }
                }
            }

            h2 { "Dialect response vs. normalized record" }
            p { "The single most educational thing in the product: what the merchant received, next to what acme actually stored." }
            div.compare {
                div { h3 { "Acme Pay response" } (components::json_pretty(&dialect_json)) }
                div { h3 { "Normalized record" } (components::json_pretty(&normalized_json)) }
            }
        }
    })
}
