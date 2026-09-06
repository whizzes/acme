//! Shared page chrome (spec §13.2/§13.5): the sandbox band, the clock
//! header, and the provider-organized rail. The colour/type tokens
//! themselves live in `static/app.css` — this module only owns structure.

use maud::{DOCTYPE, Markup, html};

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum NavItem {
    Overview,
    Payments,
    Shipments,
    Traffic,
}

pub struct Ctx<'a> {
    pub title: &'a str,
    pub clock_label: &'a str,
    pub active: NavItem,
}

/// Renders the page shell around `content`. The clock header is
/// deliberately read-only in M3 — `POST /sim/clock` and the transport
/// controls belong to the Simulator page, scoped to M6 by spec §20/§13.6.
pub fn layout(ctx: &Ctx, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (ctx.title) " · acme" }
                link rel="stylesheet" href="/static/app.css";
                script src="/static/htmx.min.js" {}
                script src="/static/htmx-ext-sse.js" {}
            }
            body hx-ext="sse" {
                div.sandbox-band { "SANDBOX — all data is simulated. Nothing here is real money or a real parcel." }
                header.clock {
                    span.clock-value { (ctx.clock_label) }
                    span.clock-hint { "sim time — clock controls arrive with the Simulator page" }
                }
                div.shell {
                    nav.rail { (rail(ctx.active)) }
                    main { (content) }
                }
                script src="/static/app.js" {}
            }
        }
    }
}

fn rail_link(item: NavItem, active: NavItem, href: &str, label: &str) -> Markup {
    let class = if item == active {
        "rail__link rail__link--active"
    } else {
        "rail__link"
    };
    html! { a class=(class) href=(href) { (label) } }
}

fn rail(active: NavItem) -> Markup {
    html! {
        div.rail__section {
            (rail_link(NavItem::Overview, active, "/", "Overview"))
        }
        div.rail__group {
            div.rail__label { "PAYMENTS" }
            (rail_link(NavItem::Payments, active, "/payments", "Acme Pay"))
        }
        div.rail__group {
            div.rail__label { "SHIPPING" }
            (rail_link(NavItem::Shipments, active, "/shipments", "Acme Ship"))
        }
        div.rail__section {
            (rail_link(NavItem::Traffic, active, "/traffic", "Traffic"))
            a.rail__link href="/docs" { "API docs" }
        }
    }
}
