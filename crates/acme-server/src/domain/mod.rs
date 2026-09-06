//! Core domain: ids, money, addresses, both state machines, events, errors
//! (spec §7, §8).

pub mod address;
pub mod error;
pub mod event;
pub mod ids;
pub mod money;
pub mod payment;
pub mod scenario;
pub mod shipment;
pub mod webhook;

use chrono::{DateTime, Utc};

use crate::domain::event::EventType;

/// Result of a state machine's `apply`: the new status, the event it
/// produced, and — for the ticker — when the next transition is due.
pub struct Transition<S> {
    pub to: S,
    pub event: EventType,
    pub occurred_at: DateTime<Utc>,
    pub next_transition_at: Option<DateTime<Utc>>,
}
