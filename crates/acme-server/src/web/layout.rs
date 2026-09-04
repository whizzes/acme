//! Shared page chrome: header with brand, nav, and the sim-clock readout.

use maud::{DOCTYPE, Markup, html};

/// Renders the page shell around `content`.
pub fn layout(title: &str, clock_label: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — acme" }
                link rel="stylesheet" href="/static/app.css";
            }
            body {
                header class="app-header" {
                    div class="app-header__brand" { "acme" }
                    nav class="app-header__nav" {
                        a href="/" { "Overview" }
                        a href="/docs" { "API Docs" }
                    }
                    div class="app-header__clock" {
                        span class="clock-label" { "sim time" }
                        span class="clock-value" { (clock_label) }
                    }
                }
                main class="app-main" {
                    (content)
                }
            }
        }
    }
}
