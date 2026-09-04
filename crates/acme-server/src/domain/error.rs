//! Errors internal to the state machines (spec §8.1/§8.2's `apply`).

use crate::domain::payment::{PaymentCommand, PaymentStatus};
use crate::domain::shipment::{ShipmentCommand, ShipmentStatus};

#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("illegal payment transition: {from:?} cannot receive {command:?}")]
    IllegalPaymentTransition {
        from: PaymentStatus,
        command: PaymentCommand,
    },
    #[error("illegal shipment transition: {from:?} cannot receive {command:?}")]
    IllegalShipmentTransition {
        from: ShipmentStatus,
        command: ShipmentCommand,
    },
}
