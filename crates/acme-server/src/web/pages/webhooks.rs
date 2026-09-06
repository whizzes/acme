//! `/webhooks` list, endpoint detail, delivery detail with the signature
//! pane (spec §13.3/§13.6, §21.9, specs/005-Webhooks.md).

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use maud::{Markup, html};

use crate::db::repo::traffic;
use crate::db::repo::webhooks::{self, DeliveryDetail, DeliveryRow, EndpointSummary};
use crate::domain::ids::{WebhookDeliveryId, WebhookEndpointId};
use crate::error::AppError;
use crate::state::AppState;
use crate::web::layout::{Ctx, NavItem, layout};
use crate::web::pages::components;

pub async fn list(State(state): State<AppState>) -> Result<Markup, AppError> {
    let clock_label = components::timestamp(state.clock.now());
    let rows = webhooks::list_endpoints(&state.db).await?;

    Ok(layout(
        &Ctx {
            title: "Webhooks",
            clock_label: &clock_label,
            active: NavItem::Webhooks,
        },
        html! {
            h1 { "Webhooks" }
            table.data {
                thead { tr {
                    th { "Endpoint" } th { "Provider" } th { "Status" } th.num { "Succeeded" } th.num { "Failed" } th { "" }
                } }
                tbody #rows {
                    @for row in &rows { (endpoint_row(row)) }
                }
            }
            @if rows.is_empty() {
                (components::empty_state(
                    "No webhook endpoints yet. Register one with the sample request for Acme Pay.",
                    Some("curl -X POST http://localhost:2263/acmepay/v1/webhook_endpoints \\\n  -H 'Authorization: Bearer sk_test_acmepay_demo' \\\n  -H 'Content-Type: application/json' \\\n  -d '{\"url\": \"https://example.test/hooks\", \"enabled_events\": [\"*\"]}'"),
                ))
            }
        },
    ))
}

fn endpoint_row(row: &EndpointSummary) -> Markup {
    let (tone, label) = if row.active {
        ("ok", "Active")
    } else {
        ("bad", "Disabled")
    };
    html! {
        tr {
            td.mono { a href={ "/webhooks/endpoints/" (row.id) } { (row.url.clone()) } }
            td { (row.provider_slug.clone()) }
            td {
                (components::pill(tone, label))
                @if row.consecutive_failures > 0 { span.hint { " " (row.consecutive_failures) " consecutive failures" } }
            }
            td.num { (row.succeeded_count) }
            td.num { (row.failed_count) }
            td {
                form hx-post={ "/webhooks/endpoints/" (row.id) "/toggle" } hx-target="closest tr" hx-swap="outerHTML" {
                    button type="submit" { @if row.active { "Disable" } @else { "Enable" } }
                }
            }
        }
    }
}

pub async fn toggle_row(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Markup, AppError> {
    let Ok(id) = id.parse::<WebhookEndpointId>() else {
        return Ok(html! { p { "not found" } });
    };
    let Some(current) = webhooks::get_unscoped(&state.db, id).await? else {
        return Ok(html! { p { "not found" } });
    };
    webhooks::set_active(&state.db, id, !current.active).await?;

    let rows = webhooks::list_endpoints(&state.db).await?;
    let Some(row) = rows.into_iter().find(|r| r.id == id) else {
        return Ok(html! { p { "not found" } });
    };
    Ok(endpoint_row(&row))
}

pub async fn endpoint_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let Ok(id) = id.parse::<WebhookEndpointId>() else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let Some(endpoint) = webhooks::get_unscoped(&state.db, id).await? else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let deliveries = webhooks::list_deliveries_for_endpoint(&state.db, id).await?;
    let clock_label = components::timestamp(state.clock.now());
    let (tone, label) = if endpoint.active {
        ("ok", "Active")
    } else {
        ("bad", "Disabled")
    };

    Ok(layout(
        &Ctx {
            title: "Webhook endpoint",
            clock_label: &clock_label,
            active: NavItem::Webhooks,
        },
        html! {
            div #endpoint-detail {
                div.detail-header {
                    h1 { (endpoint.url.clone()) }
                    (components::pill(tone, label))
                    span { (endpoint.provider_slug.clone()) }
                }
                p.hint { "Enable/disable this endpoint from the " a href="/webhooks" { "webhooks list" } "." }
                dl.kv {
                    dt { "consecutive failures" } dd { (endpoint.consecutive_failures) }
                    dt { "enabled events" } dd.mono { (endpoint.enabled_events.clone()) }
                    dt { "created" } dd.mono { (components::timestamp(endpoint.created_at)) }
                }

                h2 { "Deliveries" }
                table.data {
                    thead { tr { th { "event" } th { "status" } th.num { "attempt" } th.mono { "next attempt" } th.mono { "created" } } }
                    tbody { (delivery_rows(&deliveries)) }
                }
                @if deliveries.is_empty() {
                    (components::empty_state("No deliveries yet — they appear here as soon as a matching event happens.", None))
                }
            }
        },
    )
    .into_response())
}

fn delivery_status_tone(status: &str) -> &'static str {
    match status {
        "succeeded" => "ok",
        "pending" | "delivering" => "warn",
        "cancelled" => "idle",
        _ => "bad",
    }
}

fn delivery_rows(rows: &[DeliveryRow]) -> Markup {
    html! {
        @for row in rows {
            @let (tone, label) = (delivery_status_tone(&row.status), row.status.as_str());
            tr {
                td.mono { a href={ "/webhooks/deliveries/" (row.id) } { (row.event_type.clone()) } }
                td { (components::pill(tone, label)) }
                td.num { (row.attempt) "/" (row.max_attempts) }
                td.mono {
                    @if let Some(next) = row.next_attempt_at { (components::timestamp(next)) } @else { "—" }
                }
                td.mono { (components::timestamp(row.created_at)) }
            }
        }
    }
}

pub async fn delivery_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let Ok(id) = id.parse::<WebhookDeliveryId>() else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let clock_label = components::timestamp(state.clock.now());
    let Some(fragment) = delivery_fragment(&state, id).await? else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };

    Ok(layout(
        &Ctx {
            title: "Delivery",
            clock_label: &clock_label,
            active: NavItem::Webhooks,
        },
        fragment,
    )
    .into_response())
}

/// The whole delivery-detail body, re-rendered wholesale after retry/resend
/// (mirrors `web::pages::payments::detail_fragment`'s "swap the whole
/// detail" pattern — a delivery has no row of its own on this page).
pub async fn delivery_fragment(
    state: &AppState,
    id: WebhookDeliveryId,
) -> anyhow::Result<Option<Markup>> {
    let Some(delivery) = webhooks::get_delivery(&state.db, id).await? else {
        return Ok(None);
    };
    let attempts = traffic::list_by_delivery(&state.db, id).await?;
    let payload_bytes = match &delivery.payload_body_id {
        Some(body_id) => webhooks::read_payload_body(&state.db, body_id).await?,
        None => Vec::new(),
    };
    let payload_text = String::from_utf8_lossy(&payload_bytes).into_owned();

    let (tone, label) = (
        delivery_status_tone(&delivery.status),
        delivery.status.as_str(),
    );
    let can_retry = matches!(delivery.status.as_str(), "pending" | "failed" | "exhausted");
    let can_resend = !attempts.is_empty();

    Ok(Some(html! {
        div #delivery-detail {
            div.detail-header {
                h1 { (delivery.event_type.clone()) }
                (components::pill(tone, label))
                span.mono { (delivery.id) }
            }

            div.action-bar {
                @if can_retry {
                    form hx-post={ "/webhooks/deliveries/" (id) "/retry" } hx-target="#delivery-detail" hx-swap="outerHTML" {
                        button type="submit" { "Retry now" }
                    }
                }
                @if can_resend {
                    form hx-post={ "/webhooks/deliveries/" (id) "/resend" } hx-target="#delivery-detail" hx-swap="outerHTML" {
                        button type="submit" { "Resend as originally signed" }
                    }
                }
            }

            dl.kv {
                dt { "endpoint" } dd.mono { a href={ "/webhooks/endpoints/" (delivery.endpoint_id) } { (delivery.endpoint_url.clone()) } }
                dt { "provider" } dd { (delivery.provider_slug.clone()) }
                dt { "attempts" } dd { (delivery.attempt) "/" (delivery.max_attempts) }
                dt { "trace" } dd.mono { a href={ "/traffic/traces/" (delivery.trace_id) } { (delivery.trace_id) } }
                dt { "next attempt" } dd.mono {
                    @if let Some(next) = delivery.next_attempt_at { (components::timestamp(next)) } @else { "—" }
                }
                dt { "created" } dd.mono { (components::timestamp(delivery.created_at)) }
            }

            @if let Some(last) = attempts.last() {
                (signature_pane(&delivery, last, &payload_text))
            }

            h2 { "Attempts" }
            @if attempts.is_empty() {
                p { "No attempts yet — the dispatcher polls once per second." }
            } @else {
                table.data {
                    thead { tr { th.num { "#" } th { "status" } th.num { "response" } th.num { "took" } th.mono { "at" } } }
                    tbody {
                        @for attempt in &attempts {
                            tr {
                                td.num { @if let Some(n) = attempt.attempt { (n) } @else { "—" } }
                                td { (attempt.outcome.clone()) }
                                td.num { @if let Some(code) = attempt.status_code { (code) } @else { "—" } }
                                td.num { @if let Some(ms) = attempt.duration_ms { (ms) "ms" } @else { "—" } }
                                td.mono { (components::timestamp(attempt.started_at)) }
                            }
                        }
                    }
                }
            }

            h2 { "Payload" }
            (components::json_pretty(&payload_text))
        }
    }))
}

/// spec §21.9: scheme, timestamp, secret (revealed on demand), the exact
/// signed string, the signature sent, the tolerance window, and a
/// ready-to-run verification snippet per language.
fn signature_pane(
    delivery: &DeliveryDetail,
    last: &traffic::ExchangeRow,
    _payload_text: &str,
) -> Markup {
    let scheme = crate::domain::webhook::SigningScheme::for_provider(&delivery.provider_slug);
    let signed_payload = last.signed_payload.clone().unwrap_or_default();
    let signature = last.signature.clone().unwrap_or_default();
    let timestamp = signed_payload
        .split_once('.')
        .and_then(|(t, _)| t.parse::<i64>().ok())
        .unwrap_or(0);
    let raw_body = signed_payload
        .split_once('.')
        .map(|(_, b)| b)
        .unwrap_or_default();

    html! {
        h2 { "Signature" }
        dl.kv {
            dt { "header" } dd.mono { (scheme.header_name()) }
            dt { "timestamp" } dd.mono { (timestamp) " (" (chrono::DateTime::from_timestamp(timestamp, 0).map(|d| d.to_rfc3339()).unwrap_or_default()) ")" }
            dt { "endpoint secret" } dd {
                details {
                    summary { "reveal" }
                    span.mono { (delivery.endpoint_secret.clone()) }
                }
            }
            dt { "signed string" } dd.mono { pre.json-body { (signed_payload.clone()) } }
            dt { "signature sent" } dd.mono { (signature.clone()) }
            dt { "tolerance" } dd { "300s" }
        }

        h3 { "Verify it yourself" }
        (verification_snippets(&delivery.endpoint_secret, timestamp, raw_body, &signature))
    }
}

fn verification_snippets(secret: &str, timestamp: i64, raw_body: &str, signature: &str) -> Markup {
    let node = format!(
        "const crypto = require('crypto');\nconst signedString = `{timestamp}.${{rawBody}}`;\nconst expected = crypto.createHmac('sha256', '{secret}').update(signedString).digest('hex');\nconsole.log(expected === '{signature}');"
    );
    let python = format!(
        "import hmac, hashlib\nsigned_string = f'{timestamp}.' + raw_body\nexpected = hmac.new(b'{secret}', signed_string.encode(), hashlib.sha256).hexdigest()\nprint(expected == '{signature}')"
    );
    let php = format!(
        "$signed_string = '{timestamp}.' . $raw_body;\n$expected = hash_hmac('sha256', $signed_string, '{secret}');\nvar_dump(hash_equals($expected, '{signature}'));"
    );
    let rust = format!(
        "let signed = format!(\"{timestamp}.{{raw_body}}\");\nlet expected = hmac_sha256_hex(\"{secret}\", &signed);\nassert_eq!(expected, \"{signature}\");"
    );

    html! {
        div.compare {
            div { h4 { "Node" } pre.json-body { (node) } }
            div { h4 { "Python" } pre.json-body { (python) } }
            div { h4 { "PHP" } pre.json-body { (php) } }
            div { h4 { "Rust" } pre.json-body { (rust) } }
        }
        p.hint { "raw_body is this delivery's payload, exactly as shown below — " (raw_body.len()) " bytes." }
    }
}
