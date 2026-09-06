//! `GET /events/stream` (spec §13.4 pattern 2): the server pushes ready-
//! rendered Maud fragments over SSE, so the browser does no templating.
//! Fed by `dashboard::activity::Hub`, which both `capture::recorder` and
//! the payment/shipment repos publish onto (specs/004-Dashboard.md's Open
//! Questions).

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use maud::{Markup, html};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::dashboard::activity::ActivityEvent;
use crate::state::AppState;
use crate::web::pages::components;

pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.activity.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        // A lagging subscriber (channel overrun) just misses those events —
        // it never sees an error on the wire, matching `capture::recorder`'s
        // own "drop, don't block" policy.
        result.ok().map(|event| {
            Ok(Event::default()
                .event("activity")
                .data(render(&event).into_string()))
        })
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn render(event: &ActivityEvent) -> Markup {
    html! {
        li {
            span.ts { (components::timestamp(event.sim_at)) }
            " " (event.kind) " " (event.summary)
        }
    }
}
