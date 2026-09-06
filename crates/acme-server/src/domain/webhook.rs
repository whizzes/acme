//! Outbound webhook signing, envelope construction, and the retry
//! schedule (spec §12.1/§12.2, specs/005-Webhooks.md).

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::Rng;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// `0s -> 30s -> 2m -> 10m -> 1h -> 6h` (spec §12.1). Index 0 is the delay
/// before the *first* attempt (always immediate); index `n` is the delay
/// scheduled after attempt `n` fails. Its length is `max_attempts`.
pub const RETRY_SCHEDULE_SECONDS: [i64; 6] = [0, 30, 120, 600, 3600, 21_600];

pub fn max_attempts() -> i64 {
    RETRY_SCHEDULE_SECONDS.len() as i64
}

/// The next `next_attempt_at` after attempt number `attempt_number`
/// (1-indexed) has just failed, ±20% jitter (spec §12.1). `None` once
/// `attempt_number` has reached `max_attempts` — the delivery is exhausted.
pub fn next_retry_at(now: DateTime<Utc>, attempt_number: i64) -> Option<DateTime<Utc>> {
    let idx = usize::try_from(attempt_number).ok()?;
    let base = *RETRY_SCHEDULE_SECONDS.get(idx)?;
    let jitter = rand::thread_rng().gen_range(0.8..1.2);
    let secs = (base as f64 * jitter) as i64;
    Some(now + chrono::Duration::seconds(secs))
}

/// Keyed by `provider_slug`, matching spec §12.2's table. Only Acme Pay and
/// Acme Ship exist before M5, and both share this scheme — the enum has one
/// variant on purpose; M5 adds one arm per dialect it introduces, not a
/// redesign (specs/005-Webhooks.md item 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningScheme {
    /// `Acme-Signature: t=…,v1=…` over `{t}.{raw_body}`, hex HMAC-SHA256.
    Acme,
}

pub struct Signed {
    /// The exact bytes signed, as sent over the wire (spec §21.9: "the
    /// signed string is shown as exact bytes").
    pub signed_string: String,
    /// The full header value, e.g. `t=1773476471,v1=5257a8…`.
    pub header_value: String,
    /// Just the hex signature, e.g. `5257a8…`.
    pub signature: String,
}

impl SigningScheme {
    /// Every `provider_slug` reachable before M5 is Acme Pay or Acme Ship,
    /// both on this scheme; an unrecognized slug falls back to it too,
    /// since no other dialect exists yet to disagree.
    pub fn for_provider(_provider_slug: &str) -> Self {
        SigningScheme::Acme
    }

    pub fn header_name(self) -> &'static str {
        match self {
            SigningScheme::Acme => "Acme-Signature",
        }
    }

    pub fn sign(self, secret: &str, timestamp: i64, body: &[u8]) -> Signed {
        match self {
            SigningScheme::Acme => {
                let mut signed_bytes = format!("{timestamp}.").into_bytes();
                signed_bytes.extend_from_slice(body);

                let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                    .expect("HMAC-SHA256 accepts a key of any length");
                mac.update(&signed_bytes);
                let signature = hex::encode(mac.finalize().into_bytes());

                Signed {
                    signed_string: String::from_utf8_lossy(&signed_bytes).into_owned(),
                    header_value: format!("t={timestamp},v1={signature}"),
                    signature,
                }
            }
        }
    }

    /// Reconstructs the header value for a **resend** — the original
    /// timestamp and signature, never recomputed (spec §21.9's "Resend as
    /// originally signed").
    pub fn header_value_for(self, timestamp: i64, signature: &str) -> String {
        match self {
            SigningScheme::Acme => format!("t={timestamp},v1={signature}"),
        }
    }
}

/// The Stripe-shaped envelope every webhook body wraps its resource
/// snapshot in (spec §21.9's own example payload: `{"id":"evt_…",
/// "object":"event","type":"chec…`). `resource` is the `events.data`
/// snapshot already captured at the moment the event happened — see
/// `db::repo::payments`/`shipments`' `append_event` for why that snapshot,
/// not the resource's current row, is what gets wrapped here.
pub fn build_envelope(
    event_id: &str,
    event_type: &str,
    created_at: DateTime<Utc>,
    resource: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "id": event_id,
        "object": "event",
        "type": event_type,
        "created_at": created_at.to_rfc3339(),
        "livemode": false,
        "data": { "object": resource },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-computed against an independent HMAC-SHA256 implementation —
    /// spec §20's "Definition of done for a provider" item 5 asks for
    /// exactly this kind of check for every dialect's signing.
    #[test]
    fn acme_signing_matches_a_hand_computed_fixture() {
        // Independently verified: `printf '1700000000.{"id":"evt_1"}' |
        // openssl dgst -sha256 -hmac "whsec_test"`.
        let signed = SigningScheme::Acme.sign("whsec_test", 1_700_000_000, b"{\"id\":\"evt_1\"}");
        assert_eq!(signed.signed_string, "1700000000.{\"id\":\"evt_1\"}");
        assert_eq!(
            signed.signature,
            "c89214b5b5da833daed6f0b8c5bb6bd58cea9022bd80ccc78230f3942d632925"
        );
        assert_eq!(
            signed.header_value,
            "t=1700000000,v1=c89214b5b5da833daed6f0b8c5bb6bd58cea9022bd80ccc78230f3942d632925"
        );
    }

    #[test]
    fn signing_is_deterministic_for_the_same_inputs() {
        let a = SigningScheme::Acme.sign("secret", 1, b"body");
        let b = SigningScheme::Acme.sign("secret", 1, b"body");
        assert_eq!(a.signature, b.signature);
    }

    #[test]
    fn different_secrets_produce_different_signatures() {
        let a = SigningScheme::Acme.sign("secret-a", 1, b"body");
        let b = SigningScheme::Acme.sign("secret-b", 1, b"body");
        assert_ne!(a.signature, b.signature);
    }

    #[test]
    fn retry_schedule_exhausts_after_max_attempts() {
        let now = Utc::now();
        assert!(next_retry_at(now, max_attempts()).is_none());
        assert!(next_retry_at(now, max_attempts() - 1).is_some());
    }
}
