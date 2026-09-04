//! The M0 dashboard landing page.

use maud::{Markup, html};

use crate::web::layout::layout;

/// Renders the overview page.
pub fn render(clock_label: &str) -> Markup {
    layout(
        "Overview",
        clock_label,
        html! {
            section class="empty-state" {
                h1 { "acme" }
                p { "Skeleton is up. Payments, shipments and webhooks arrive in later milestones." }
            }
        },
    )
}
