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

impl EventType {
    /// Human-readable label for a shipment tracking event (spec §11.1's
    /// "normalized event list"). Payment events don't need one yet — only
    /// `shipment_events.description` surfaces this to a caller in M2.
    pub fn description(self) -> &'static str {
        use EventType::*;
        match self {
            ShipmentCreated => "Shipment registered",
            ShipmentLabelGenerated => "Label generated",
            ShipmentPickedUp => "Picked up",
            ShipmentInTransit => "In transit",
            ShipmentAtFacility => "Arrived at facility",
            ShipmentOutForDelivery => "Out for delivery",
            ShipmentDeliveryAttempted => "Delivery attempted",
            ShipmentDelivered => "Delivered",
            ShipmentException => "Exception",
            ShipmentReturning => "Returning to sender",
            ShipmentReturned => "Returned to sender",
            ShipmentCancelled => "Cancelled",
            ShipmentLost => "Lost",
            _ => self.as_str(),
        }
    }
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
