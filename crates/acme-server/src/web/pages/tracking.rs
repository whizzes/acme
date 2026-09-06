//! `GET /t/{provider}/{tracking}` (spec §13.3, specs/008-Simulation.md
//! item 7): a public, carrier-styled tracking page — no dashboard chrome,
//! no auth, built entirely on the same `shipments::get_by_tracking`/
//! `list_events` query every shipping dialect's own tracking endpoint
//! already calls (`acmeship`'s `public_tracking`, `iberex`'s
//! `seguimiento_expedicion`).

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use maud::{Markup, html};

use crate::db::repo::shipments;
use crate::error::AppError;
use crate::state::AppState;
use crate::web::pages::components;

/// `(display name, accent color)` per carrier — a small `const` table,
/// not a bespoke template per dialect (specs/008-Simulation.md item 7).
fn carrier_style(carrier_code: &str) -> (&'static str, &'static str) {
    match carrier_code {
        "acmeship" => ("Acme Ship", "#2f6fed"),
        "iberex" => ("Iberex Express", "#e30613"),
        _ => ("Carrier", "#555555"),
    }
}

pub async fn page(
    State(state): State<AppState>,
    Path((provider, tracking)): Path<(String, String)>,
) -> Result<axum::response::Response, AppError> {
    let Some(row) = shipments::get_by_tracking(&state.db, &provider, &tracking).await? else {
        return Ok((
            axum::http::StatusCode::NOT_FOUND,
            shell("Tracking", html! { p { "No shipment found for that tracking number." } }),
        )
            .into_response());
    };
    let events = shipments::list_events(&state.db, row.id).await?;
    let (carrier_name, accent) = carrier_style(&row.carrier_code);

    Ok(shell(
        carrier_name,
        html! {
            header style={ "background:" (accent) } {
                span.carrier-name { (carrier_name) }
            }
            main.tracking-body {
                h1.mono { (row.tracking_number.clone()) }
                @let (tone, label) = components::shipment_tone(row.status);
                p.tracking-status { (components::pill(tone, label)) }
                @if let Some(eta) = row.eta_at {
                    p.hint { "Estimated delivery: " (components::timestamp(eta)) }
                }
                ol.tracking-timeline {
                    @for event in events.iter().rev() {
                        li {
                            span.ts { (components::timestamp(event.occurred_at)) }
                            span { (event.description.clone()) }
                        }
                    }
                }
            }
        },
    )
    .into_response())
}

/// Deliberately *not* `web::layout::layout` — no rail, no sandbox band's
/// dashboard styling, matching spec §13.7's hosted-checkout precedent of
/// a page styled unlike the operator dashboard on purpose.
fn shell(title: &str, body: Markup) -> Markup {
    html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · tracking" }
                link rel="stylesheet" href="/static/tracking.css";
            }
            body.tracking-page { (body) }
        }
    }
}
