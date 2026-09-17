//! The Overview page (spec §13.3/§13.2): counters, sparklines, and the
//! live activity feed.

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use maud::{Markup, html};

use crate::db::repo::{dashboard, traffic};
use crate::state::AppState;
use crate::web::layout::{Ctx, NavItem, layout};
use crate::web::pages::components;

const SPARKLINE_HOURS: usize = 6;

pub async fn render(state: &AppState) -> anyhow::Result<Markup> {
    let counters = dashboard::counters(&state.db).await?;
    let now = state.clock.now();
    let since = now - Duration::hours(SPARKLINE_HOURS as i64);

    let captured_series = dashboard::payments_captured_by_hour(&state.db, since).await?;
    let delivered_series = dashboard::shipments_delivered_by_hour(&state.db, since).await?;
    let captured_buckets = bucketize(&captured_series, now);
    let delivered_buckets = bucketize(&delivered_series, now);

    let recent = traffic::list(
        &state.db,
        &traffic::ListFilter {
            limit: 20,
            ..Default::default()
        },
    )
    .await?;

    let clock_label = components::timestamp(now);

    Ok(layout(
        &Ctx {
            title: "Overview",
            clock_label: &clock_label,
            active: NavItem::Overview,
        },
        html! {
            h1 { "acme" }

            div.kpi-row {
                div.kpi {
                    div.kpi-label { "Payments" }
                    div.kpi-value { (counters.payments_total) }
                }
                div.kpi {
                    div.kpi-label { "captured, last " (SPARKLINE_HOURS) "h" }
                    (components::sparkline(&captured_buckets))
                }
                div.kpi {
                    div.kpi-label { "Shipments" }
                    div.kpi-value { (counters.shipments_total) }
                }
                div.kpi {
                    div.kpi-label { "delivered, last " (SPARKLINE_HOURS) "h" }
                    (components::sparkline(&delivered_buckets))
                }
                div.kpi {
                    div.kpi-label { "Webhook endpoints" }
                    div.kpi-value { (counters.webhook_endpoints_total) }
                }
            }

            h2 { "Recent activity" }
            @if recent.is_empty() {
                (components::empty_state(
                    "No traffic yet.",
                    Some(("/providers/acmepay", "Create a payment on the Acme Pay provider page")),
                ))
            }
            div hx-ext="sse" sse-connect="/events/stream" {
                ul #feed.live-feed sse-swap="activity" hx-swap="afterbegin settle:200ms" {
                    @for row in recent.iter().rev() {
                        (exchange_feed_row(row))
                    }
                }
            }
        },
    ))
}

fn exchange_feed_row(row: &traffic::ExchangeRow) -> Markup {
    html! {
        li {
            span.ts { (components::timestamp(row.sim_at)) }
            " " (row.direction) " " (row.method) " " (row.path)
            @if let Some(code) = row.status_code { " " (code) }
        }
    }
}

/// Aligns a sparse hourly series against a fixed `SPARKLINE_HOURS`-wide
/// window ending at `now`, zero-filling any hour with no rows.
fn bucketize(series: &[dashboard::HourlyPoint], now: DateTime<Utc>) -> Vec<i64> {
    let mut buckets = vec![0i64; SPARKLINE_HOURS];
    for point in series {
        let Ok(naive) = NaiveDateTime::parse_from_str(&point.hour, "%Y-%m-%dT%H:%M:%S") else {
            continue;
        };
        let hour_dt = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
        let diff = (now - hour_dt).num_hours();
        if (0..SPARKLINE_HOURS as i64).contains(&diff) {
            buckets[SPARKLINE_HOURS - 1 - diff as usize] += point.count;
        }
    }
    buckets
}
