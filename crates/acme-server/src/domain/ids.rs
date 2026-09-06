//! Prefixed ULID identifiers (e.g. `pay_01J...`), sortable and self-describing
//! in logs (spec §7).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Debug, thiserror::Error)]
pub enum IdParseError {
    #[error("expected `{expected}_` prefix, got `{actual}`")]
    WrongPrefix {
        expected: &'static str,
        actual: String,
    },
    #[error("invalid ULID: {0}")]
    InvalidUlid(#[from] ulid::DecodeError),
}

/// Defines a newtype wrapping a `Ulid`, rendered as `{prefix}_{ulid}`.
/// Stored as `TEXT` in SQLite via `Display`/`FromStr`, not a custom
/// `sqlx::Type` impl — repo functions bind/read the string form directly.
macro_rules! prefixed_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Ulid);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            pub fn new() -> Self {
                Self(Ulid::new())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}_{}", Self::PREFIX, self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let rest = s.strip_prefix(concat!($prefix, "_")).ok_or_else(|| {
                    IdParseError::WrongPrefix {
                        expected: $prefix,
                        actual: s.to_string(),
                    }
                })?;
                Ok(Self(Ulid::from_str(rest)?))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                Self::from_str(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

prefixed_id!(MerchantId, "mrc");
prefixed_id!(CustomerId, "cus");
prefixed_id!(AddressId, "adr");
prefixed_id!(OrderId, "ord");
prefixed_id!(PaymentId, "pay");
prefixed_id!(CheckoutSessionId, "cs");
prefixed_id!(RefundId, "ref");
prefixed_id!(DisputeId, "dsp");
prefixed_id!(ShipmentId, "shp");
prefixed_id!(RateQuoteId, "qte");
prefixed_id!(RateOptionId, "rto");
prefixed_id!(PickupId, "pck");
prefixed_id!(EventId, "evt");
prefixed_id!(WebhookEndpointId, "whe");
prefixed_id!(WebhookDeliveryId, "whd");
prefixed_id!(ExchangeId, "ex");
prefixed_id!(TraceId, "trc");
prefixed_id!(FaultId, "flt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_display_and_from_str() {
        let id = PaymentId::new();
        let rendered = id.to_string();
        assert!(rendered.starts_with("pay_"));
        assert_eq!(rendered.parse::<PaymentId>().unwrap(), id);
    }

    #[test]
    fn rejects_wrong_prefix() {
        let shp = ShipmentId::new().to_string();
        assert!(shp.parse::<PaymentId>().is_err());
    }

    #[test]
    fn round_trips_through_serde() {
        let id = ShipmentId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: ShipmentId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }
}
