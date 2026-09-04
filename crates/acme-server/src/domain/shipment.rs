//! Shipment state machine and happy-path event schedule (spec §8.2).

use chrono::Duration;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::domain::error::DomainError;
use crate::domain::event::EventType;
use crate::sim::clock::SimClock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum ShipmentStatus {
    Quoted,
    Created,
    LabelGenerated,
    PickedUp,
    InTransit,
    AtFacility,
    OutForDelivery,
    DeliveryAttempted,
    Delivered,
    Exception,
    Returning,
    Returned,
    Cancelled,
    Lost,
}

impl ShipmentStatus {
    pub const ALL: [ShipmentStatus; 14] = [
        ShipmentStatus::Quoted,
        ShipmentStatus::Created,
        ShipmentStatus::LabelGenerated,
        ShipmentStatus::PickedUp,
        ShipmentStatus::InTransit,
        ShipmentStatus::AtFacility,
        ShipmentStatus::OutForDelivery,
        ShipmentStatus::DeliveryAttempted,
        ShipmentStatus::Delivered,
        ShipmentStatus::Exception,
        ShipmentStatus::Returning,
        ShipmentStatus::Returned,
        ShipmentStatus::Cancelled,
        ShipmentStatus::Lost,
    ];

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ShipmentStatus::Delivered
                | ShipmentStatus::Returned
                | ShipmentStatus::Cancelled
                | ShipmentStatus::Lost
        )
    }

    pub fn can_transition_to(self, next: ShipmentStatus) -> bool {
        ShipmentCommand::ALL_KINDS
            .iter()
            .any(|cmd| target(self, cmd) == Some(next))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShipmentCommand {
    Create,
    GenerateLabel,
    PickUp,
    Depart,
    ArriveAtFacility,
    DepartFacility,
    Dispatch,
    Deliver,
    AttemptDelivery,
    RetryDelivery,
    GiveUpDelivery,
    Except,
    StartReturn,
    CompleteReturn,
    MarkLost,
    Cancel,
}

impl ShipmentCommand {
    pub const ALL_KINDS: [ShipmentCommand; 16] = [
        ShipmentCommand::Create,
        ShipmentCommand::GenerateLabel,
        ShipmentCommand::PickUp,
        ShipmentCommand::Depart,
        ShipmentCommand::ArriveAtFacility,
        ShipmentCommand::DepartFacility,
        ShipmentCommand::Dispatch,
        ShipmentCommand::Deliver,
        ShipmentCommand::AttemptDelivery,
        ShipmentCommand::RetryDelivery,
        ShipmentCommand::GiveUpDelivery,
        ShipmentCommand::Except,
        ShipmentCommand::StartReturn,
        ShipmentCommand::CompleteReturn,
        ShipmentCommand::MarkLost,
        ShipmentCommand::Cancel,
    ];
}

pub type Transition = crate::domain::Transition<ShipmentStatus>;

/// The transition table. `None` means the command is illegal from `cur`.
///
/// `AttemptDelivery`'s retry-vs-give-up fork (spec's "attempts < 3") is a
/// caller decision, not encoded here: the caller reads
/// `shipments.delivery_attempts` and issues `RetryDelivery` or
/// `GiveUpDelivery` accordingly. `apply` stays a pure status table.
fn target(cur: ShipmentStatus, cmd: &ShipmentCommand) -> Option<ShipmentStatus> {
    use ShipmentCommand::*;
    use ShipmentStatus::*;

    match (cur, cmd) {
        (Quoted, Create) => Some(Created),
        (Created, GenerateLabel) => Some(LabelGenerated),
        (LabelGenerated, PickUp) => Some(PickedUp),
        (PickedUp, Depart) => Some(InTransit),
        (InTransit, ArriveAtFacility) => Some(AtFacility),
        (AtFacility, DepartFacility) => Some(InTransit),
        (AtFacility, Dispatch) => Some(OutForDelivery),
        (OutForDelivery, Deliver) => Some(Delivered),
        (OutForDelivery, AttemptDelivery) => Some(DeliveryAttempted),
        (OutForDelivery, Except) => Some(Exception),
        (DeliveryAttempted, RetryDelivery) => Some(OutForDelivery),
        (DeliveryAttempted, GiveUpDelivery) => Some(Exception),
        (Exception, StartReturn) => Some(Returning),
        (Returning, CompleteReturn) => Some(Returned),
        (Exception, MarkLost) => Some(Lost),
        (Created, Cancel) => Some(Cancelled),
        (LabelGenerated, Cancel) => Some(Cancelled),
        (PickedUp, Cancel) => Some(Cancelled),
        _ => None,
    }
}

fn event_for(cmd: &ShipmentCommand) -> EventType {
    use ShipmentCommand::*;

    match cmd {
        Create => EventType::ShipmentCreated,
        GenerateLabel => EventType::ShipmentLabelGenerated,
        PickUp => EventType::ShipmentPickedUp,
        Depart => EventType::ShipmentInTransit,
        ArriveAtFacility => EventType::ShipmentAtFacility,
        DepartFacility => EventType::ShipmentInTransit,
        Dispatch => EventType::ShipmentOutForDelivery,
        Deliver => EventType::ShipmentDelivered,
        AttemptDelivery => EventType::ShipmentDeliveryAttempted,
        RetryDelivery => EventType::ShipmentOutForDelivery,
        GiveUpDelivery => EventType::ShipmentException,
        Except => EventType::ShipmentException,
        StartReturn => EventType::ShipmentReturning,
        CompleteReturn => EventType::ShipmentReturned,
        MarkLost => EventType::ShipmentLost,
        Cancel => EventType::ShipmentCancelled,
    }
}

pub fn apply(
    cur: ShipmentStatus,
    cmd: ShipmentCommand,
    clock: &SimClock,
) -> Result<Transition, DomainError> {
    let Some(to) = target(cur, &cmd) else {
        return Err(DomainError::IllegalShipmentTransition { from: cur, command: cmd });
    };

    Ok(Transition {
        to,
        event: event_for(&cmd),
        occurred_at: clock.now(),
        next_transition_at: None,
    })
}

/// One hop of a generated happy-path schedule: `at` is the offset from the
/// shipment's creation time.
#[derive(Debug, Clone, Copy)]
pub struct ScheduledHop {
    pub at: Duration,
    pub command: ShipmentCommand,
    pub status: ShipmentStatus,
    pub event: EventType,
}

/// The happy-path event schedule for a shipment (spec §8.2): quoted through
/// delivered, jittered ±25% from a seeded RNG so no two shipments look
/// identical, monotonically ordered so `next_transition_at` never runs
/// backwards. Every consecutive pair in `FRACTIONS` is a legal edge in
/// `target()` above — PickedUp -> InTransit -> AtFacility -> InTransit ->
/// AtFacility -> OutForDelivery -> Delivered — which walks one more
/// InTransit hop than spec §8.2's worked example (whose timeline skips
/// straight from picked_up to at_facility); the two aren't fully
/// reconcilable, and the state graph wins since `apply` enforces it.
pub fn happy_path_schedule(eta: Duration, seed: u64) -> Vec<ScheduledHop> {
    const FRACTIONS: [(f64, ShipmentCommand, ShipmentStatus); 7] = [
        (0.084, ShipmentCommand::PickUp, ShipmentStatus::PickedUp),
        (0.120, ShipmentCommand::Depart, ShipmentStatus::InTransit),
        (0.179, ShipmentCommand::ArriveAtFacility, ShipmentStatus::AtFacility),
        (0.383, ShipmentCommand::DepartFacility, ShipmentStatus::InTransit),
        (0.753, ShipmentCommand::ArriveAtFacility, ShipmentStatus::AtFacility),
        (0.861, ShipmentCommand::Dispatch, ShipmentStatus::OutForDelivery),
        (1.0, ShipmentCommand::Deliver, ShipmentStatus::Delivered),
    ];

    let mut rng = StdRng::seed_from_u64(seed);
    let eta_secs = eta.num_seconds() as f64;
    let min_gap = Duration::minutes(1);

    let mut hops = vec![
        ScheduledHop {
            at: Duration::zero(),
            command: ShipmentCommand::Create,
            status: ShipmentStatus::Created,
            event: EventType::ShipmentCreated,
        },
        ScheduledHop {
            at: Duration::zero(),
            command: ShipmentCommand::GenerateLabel,
            status: ShipmentStatus::LabelGenerated,
            event: EventType::ShipmentLabelGenerated,
        },
    ];

    let mut previous = Duration::zero();
    for (fraction, command, status) in FRACTIONS {
        let jitter = rng.gen_range(0.75..1.25);
        let raw = Duration::seconds((eta_secs * fraction * jitter) as i64);
        let at = raw.max(previous + min_gap);
        previous = at;
        hops.push(ScheduledHop { at, command, status, event: event_for(&command) });
    }

    hops
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::{HashSet, VecDeque};

    fn clock() -> SimClock {
        SimClock::new(Utc::now(), 60.0)
    }

    #[test]
    fn exhaustive_table_matches_apply() {
        let clock = clock();
        for &status in ShipmentStatus::ALL.iter() {
            for cmd in ShipmentCommand::ALL_KINDS.iter() {
                let expected = target(status, cmd);
                let actual = apply(status, *cmd, &clock).ok().map(|t| t.to);
                assert_eq!(
                    actual, expected,
                    "apply({status:?}, {cmd:?}) should be {expected:?}"
                );
            }
        }
    }

    #[test]
    fn terminal_states_have_no_outgoing_transitions() {
        for &status in ShipmentStatus::ALL.iter().filter(|s| s.is_terminal()) {
            for cmd in ShipmentCommand::ALL_KINDS.iter() {
                assert!(
                    target(status, cmd).is_none(),
                    "{status:?} is terminal but accepts {cmd:?}"
                );
            }
        }
    }

    #[test]
    fn reachability_from_quoted() {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([ShipmentStatus::Quoted]);
        seen.insert(ShipmentStatus::Quoted);

        while let Some(cur) = queue.pop_front() {
            for cmd in ShipmentCommand::ALL_KINDS.iter() {
                if let Some(next) = target(cur, cmd) {
                    if seen.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }

        for &status in ShipmentStatus::ALL.iter() {
            assert!(seen.contains(&status), "{status:?} is unreachable from Quoted");
        }
    }

    #[test]
    fn happy_path_schedule_is_monotonic_and_ends_at_delivered() {
        let schedule = happy_path_schedule(Duration::hours(25) + Duration::minutes(40), 42);
        let mut prev = Duration::zero();
        for hop in &schedule {
            assert!(hop.at >= prev, "schedule went backwards at {:?}", hop.status);
            prev = hop.at;
        }
        assert_eq!(schedule.last().unwrap().status, ShipmentStatus::Delivered);
    }

    #[test]
    fn happy_path_schedule_is_deterministic_for_a_seed() {
        let eta = Duration::hours(30);
        let a = happy_path_schedule(eta, 7);
        let b = happy_path_schedule(eta, 7);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.at, y.at);
        }
    }
}
