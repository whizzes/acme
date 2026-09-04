//! Payment state machine (spec §8.1). `apply` is pure and total: every
//! `(PaymentStatus, PaymentCommand)` pair either transitions or errors,
//! never panics.

use serde::{Deserialize, Serialize};

use crate::domain::error::DomainError;
use crate::domain::event::EventType;
use crate::domain::money::Money;
use crate::sim::clock::SimClock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum PaymentStatus {
    Created,
    Pending,
    Authorized,
    Captured,
    PartiallyRefunded,
    Refunded,
    Rejected,
    Cancelled,
    Expired,
    Disputed,
    ChargedBack,
}

impl PaymentStatus {
    pub const ALL: [PaymentStatus; 11] = [
        PaymentStatus::Created,
        PaymentStatus::Pending,
        PaymentStatus::Authorized,
        PaymentStatus::Captured,
        PaymentStatus::PartiallyRefunded,
        PaymentStatus::Refunded,
        PaymentStatus::Rejected,
        PaymentStatus::Cancelled,
        PaymentStatus::Expired,
        PaymentStatus::Disputed,
        PaymentStatus::ChargedBack,
    ];

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            PaymentStatus::Rejected
                | PaymentStatus::Cancelled
                | PaymentStatus::Expired
                | PaymentStatus::Refunded
                | PaymentStatus::ChargedBack
        )
    }

    pub fn can_transition_to(self, next: PaymentStatus) -> bool {
        PaymentCommand::ALL_KINDS
            .iter()
            .any(|cmd| target(self, cmd) == Some(next))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclineReason {
    InsufficientFunds,
    CardExpired,
    InvalidCvv,
    DoNotHonor,
    StolenCard,
    LimitExceeded,
    IssuerUnavailable,
    RiskRejected,
    ThreeDsFailed,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaymentCommand {
    MarkPending,
    Authorize,
    Reject(DeclineReason),
    Capture,
    Cancel,
    Expire,
    RefundPartially { amount: Money },
    Refund,
    OpenDispute,
    ChargeBack,
    WinDispute,
}

impl PaymentCommand {
    /// One representative instance per variant, for exhaustive table tests.
    pub const ALL_KINDS: [PaymentCommand; 11] = [
        PaymentCommand::MarkPending,
        PaymentCommand::Authorize,
        PaymentCommand::Reject(DeclineReason::DoNotHonor),
        PaymentCommand::Capture,
        PaymentCommand::Cancel,
        PaymentCommand::Expire,
        PaymentCommand::RefundPartially {
            amount: Money::new(0, crate::domain::money::Currency::Usd),
        },
        PaymentCommand::Refund,
        PaymentCommand::OpenDispute,
        PaymentCommand::ChargeBack,
        PaymentCommand::WinDispute,
    ];
}

pub type Transition = crate::domain::Transition<PaymentStatus>;

/// The transition table. `None` means the command is illegal from `cur`.
fn target(cur: PaymentStatus, cmd: &PaymentCommand) -> Option<PaymentStatus> {
    use PaymentCommand::*;
    use PaymentStatus::*;

    match (cur, cmd) {
        (Created, MarkPending) => Some(Pending),
        (Created, Reject(_)) => Some(Rejected),
        (Pending, Authorize) => Some(Authorized),
        (Pending, Reject(_)) => Some(Rejected),
        (Pending, Cancel) => Some(Cancelled),
        (Pending, Expire) => Some(Expired),
        (Authorized, Capture) => Some(Captured),
        (Authorized, Cancel) => Some(Cancelled),
        (Captured, RefundPartially { .. }) => Some(PartiallyRefunded),
        (PartiallyRefunded, Refund) => Some(Refunded),
        (Captured, OpenDispute) => Some(Disputed),
        (Disputed, ChargeBack) => Some(ChargedBack),
        (Disputed, WinDispute) => Some(Captured),
        _ => None,
    }
}

fn event_for(cmd: &PaymentCommand) -> EventType {
    use PaymentCommand::*;

    match cmd {
        MarkPending => EventType::PaymentPending,
        Authorize => EventType::PaymentAuthorized,
        Reject(_) => EventType::PaymentRejected,
        Cancel => EventType::PaymentCancelled,
        Expire => EventType::PaymentExpired,
        Capture => EventType::PaymentCaptured,
        RefundPartially { .. } => EventType::PaymentPartiallyRefunded,
        Refund => EventType::PaymentRefunded,
        OpenDispute => EventType::PaymentDisputed,
        ChargeBack => EventType::PaymentChargedBack,
        WinDispute => EventType::PaymentDisputeWon,
    }
}

pub fn apply(
    cur: PaymentStatus,
    cmd: PaymentCommand,
    clock: &SimClock,
) -> Result<Transition, DomainError> {
    let Some(to) = target(cur, &cmd) else {
        return Err(DomainError::IllegalPaymentTransition { from: cur, command: cmd });
    };

    Ok(Transition {
        to,
        event: event_for(&cmd),
        occurred_at: clock.now(),
        next_transition_at: None,
    })
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
        for &status in PaymentStatus::ALL.iter() {
            for cmd in PaymentCommand::ALL_KINDS.iter() {
                let expected = target(status, cmd);
                let actual = apply(status, cmd.clone(), &clock).ok().map(|t| t.to);
                assert_eq!(
                    actual, expected,
                    "apply({status:?}, {cmd:?}) should be {expected:?}"
                );
            }
        }
    }

    #[test]
    fn illegal_transitions_error_never_panic() {
        let clock = clock();
        assert!(apply(PaymentStatus::Refunded, PaymentCommand::Capture, &clock).is_err());
        assert!(apply(PaymentStatus::Created, PaymentCommand::WinDispute, &clock).is_err());
    }

    #[test]
    fn terminal_states_have_no_outgoing_transitions() {
        for &status in PaymentStatus::ALL.iter().filter(|s| s.is_terminal()) {
            for cmd in PaymentCommand::ALL_KINDS.iter() {
                assert!(
                    target(status, cmd).is_none(),
                    "{status:?} is terminal but accepts {cmd:?}"
                );
            }
        }
    }

    /// Breadth-first walk over every legal transition from `Created`: every
    /// non-terminal, non-`Created` status must actually be reachable,
    /// catching a dead branch in the table (the type system already rules
    /// out reaching anything that isn't a real `PaymentStatus` variant).
    #[test]
    fn reachability_never_leaves_the_enum() {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([PaymentStatus::Created]);
        seen.insert(PaymentStatus::Created);

        while let Some(cur) = queue.pop_front() {
            for cmd in PaymentCommand::ALL_KINDS.iter() {
                if let Some(next) = target(cur, cmd) {
                    if seen.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }

        for &status in PaymentStatus::ALL.iter() {
            assert!(seen.contains(&status), "{status:?} is unreachable from Created");
        }
    }
}
