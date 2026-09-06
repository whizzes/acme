//! In-process broadcast hub backing `GET /events/stream` (spec §13.4
//! pattern 2). Fed from two places, per specs/004-Dashboard.md's Open
//! Questions: `capture::recorder` (every captured HTTP exchange) and
//! `db::repo::payments`/`db::repo::shipments` (every state transition,
//! including ticker-driven ones with no HTTP exchange to key off).
//!
//! Reached through a process-wide `OnceLock` rather than threaded through
//! every `advance()`/`record()` call site — those already have several
//! callers each (provider handlers, the ticker), and publishing an
//! in-memory event is not a persistence concern the "repos are the only
//! code allowed to query" rule (spec §5) protects.

use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

const CHANNEL_CAPACITY: usize = 1024;

#[derive(Clone, Debug)]
pub struct ActivityEvent {
    /// `"payment"`, `"shipment"`, or `"http"` — which Overview lane this
    /// renders into.
    pub kind: &'static str,
    pub resource_type: Option<&'static str>,
    pub resource_id: Option<String>,
    pub summary: String,
    pub sim_at: DateTime<Utc>,
}

impl ActivityEvent {
    pub fn resource(
        resource_type: &'static str,
        resource_id: String,
        summary: String,
        sim_at: DateTime<Utc>,
    ) -> Self {
        Self {
            kind: resource_type,
            resource_type: Some(resource_type),
            resource_id: Some(resource_id),
            summary,
            sim_at,
        }
    }

    pub fn http(summary: String, sim_at: DateTime<Utc>) -> Self {
        Self {
            kind: "http",
            resource_type: None,
            resource_id: None,
            summary,
            sim_at,
        }
    }
}

#[derive(Clone)]
pub struct Hub(broadcast::Sender<ActivityEvent>);

impl Hub {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self(tx)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ActivityEvent> {
        self.0.subscribe()
    }

    /// No subscribers (no dashboard page open) is not an error — this
    /// mirrors `capture::recorder::record`'s "never fails the caller".
    pub fn publish(&self, event: ActivityEvent) {
        let _ = self.0.send(event);
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL: OnceLock<Hub> = OnceLock::new();

/// Called once from `run()`, before anything can publish.
pub fn init(hub: Hub) {
    let _ = GLOBAL.set(hub);
}

pub fn global() -> Option<&'static Hub> {
    GLOBAL.get()
}

/// A no-op until `init` runs — keeps `db::repo`/`capture` unit tests free
/// of any hub setup they don't otherwise care about.
pub fn publish(event: ActivityEvent) {
    if let Some(hub) = global() {
        hub.publish(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_subscribers_both_receive_a_published_event() {
        let hub = Hub::new();
        let mut a = hub.subscribe();
        let mut b = hub.subscribe();

        hub.publish(ActivityEvent::http("GET /x 200".into(), Utc::now()));

        assert_eq!(a.try_recv().unwrap().summary, "GET /x 200");
        assert_eq!(b.try_recv().unwrap().summary, "GET /x 200");
    }

    #[test]
    fn publish_without_a_global_hub_never_panics() {
        publish(ActivityEvent::http("noop".into(), Utc::now()));
    }
}
