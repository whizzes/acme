//! Canonical dialect-facing event type strings (spec §7.5 `events.type`,
//! §12.3), written by the state machines and read back by `events.data`.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    PaymentPending,
    PaymentAuthorized,
    PaymentCaptured,
    PaymentRejected,
    PaymentCancelled,
    PaymentExpired,
    PaymentRefunded,
    PaymentPartiallyRefunded,
    PaymentDisputed,
    PaymentChargedBack,
    PaymentDisputeWon,
    ShipmentCreated,
    ShipmentLabelGenerated,
    ShipmentPickedUp,
    ShipmentInTransit,
    ShipmentAtFacility,
    ShipmentOutForDelivery,
    ShipmentDeliveryAttempted,
    ShipmentDelivered,
    ShipmentException,
    ShipmentReturning,
    ShipmentReturned,
    ShipmentCancelled,
    ShipmentLost,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        use EventType::*;
        match self {
            PaymentPending => "payment.pending",
            PaymentAuthorized => "payment.authorized",
            PaymentCaptured => "payment.captured",
            PaymentRejected => "payment.rejected",
            PaymentCancelled => "payment.cancelled",
            PaymentExpired => "payment.expired",
            PaymentRefunded => "payment.refunded",
            PaymentPartiallyRefunded => "payment.partially_refunded",
            PaymentDisputed => "payment.disputed",
            PaymentChargedBack => "payment.charged_back",
            PaymentDisputeWon => "payment.dispute_won",
            ShipmentCreated => "shipment.created",
            ShipmentLabelGenerated => "shipment.label_generated",
            ShipmentPickedUp => "shipment.picked_up",
            ShipmentInTransit => "shipment.in_transit",
            ShipmentAtFacility => "shipment.at_facility",
            ShipmentOutForDelivery => "shipment.out_for_delivery",
            ShipmentDeliveryAttempted => "shipment.delivery_attempted",
            ShipmentDelivered => "shipment.delivered",
            ShipmentException => "shipment.exception",
            ShipmentReturning => "shipment.returning",
            ShipmentReturned => "shipment.returned",
            ShipmentCancelled => "shipment.cancelled",
            ShipmentLost => "shipment.lost",
        }
    }
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
