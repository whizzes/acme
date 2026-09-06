//! Maps between `db::repo::payments` rows and Trancorp Webpay's wire
//! shapes (spec §10.2): status vocabulary, `response_code`, and the
//! `MMDD`/millisecond-precision timestamp formats the real dialect uses.

use chrono::{DateTime, Utc};

use crate::domain::payment::PaymentStatus;
use crate::providers::payments::webpay::dto::{CardDetail, TransactionDetail};

/// `INITIALIZED -> AUTHORIZED -> {FAILED, NULLIFIED, REVERSED}` (spec
/// §10.2's own mapping notes). `Disputed`/`ChargedBack` have no Trancorp
/// equivalent documented — this dialect has no dispute concept, so they
/// fall back to `AUTHORIZED` rather than inventing a status spec §10.2
/// never names.
pub fn webpay_status(status: PaymentStatus) -> &'static str {
    use PaymentStatus::*;
    match status {
        Created | Pending => "INITIALIZED",
        Authorized | Captured | PartiallyRefunded | Disputed | ChargedBack => "AUTHORIZED",
        Rejected | Expired => "FAILED",
        Refunded => "NULLIFIED",
        Cancelled => "REVERSED",
    }
}

/// `0` approved, `-1` rejected — see `dto::TransactionDetail::response_code`
/// for why this sandbox trims spec §10.2's fuller table.
pub fn response_code(status: PaymentStatus) -> i32 {
    if status == PaymentStatus::Rejected {
        -1
    } else {
        0
    }
}

/// A deterministic, PAN-free "last four digits" — Trancorp never receives
/// card data through the merchant API at all (spec §10.2: card entry
/// happens purely on Transbank's own hosted page), so there is no real PAN
/// to mask. Derived from the checkout token so it's stable across commit
/// and status calls for the same transaction.
pub fn synthetic_last4(seed: &str) -> String {
    let mut hash: u32 = 2166136261;
    for byte in seed.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    format!("{:04}", hash % 10000)
}

pub fn transaction_detail(
    status: PaymentStatus,
    buy_order: String,
    session_id: String,
    amount_cents: i64,
    token: &str,
    authorized_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> TransactionDetail {
    let at = authorized_at.unwrap_or(now);
    TransactionDetail {
        vci: "TSY".to_string(),
        amount: amount_cents,
        status: webpay_status(status).to_string(),
        buy_order,
        session_id,
        card_detail: CardDetail {
            card_number: synthetic_last4(token),
        },
        accounting_date: at.format("%m%d").to_string(),
        transaction_date: at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        authorization_code: matches!(
            status,
            PaymentStatus::Authorized | PaymentStatus::Captured | PaymentStatus::PartiallyRefunded
        )
        .then(|| format!("A{}", synthetic_last4(&format!("auth{token}")))),
        payment_type_code: "VN".to_string(),
        response_code: response_code(status),
        installments_number: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_last4_is_deterministic() {
        assert_eq!(synthetic_last4("token-a"), synthetic_last4("token-a"));
    }

    #[test]
    fn response_code_is_zero_unless_rejected() {
        assert_eq!(response_code(PaymentStatus::Captured), 0);
        assert_eq!(response_code(PaymentStatus::Rejected), -1);
    }

    #[test]
    fn status_mapping_matches_spec_table() {
        assert_eq!(webpay_status(PaymentStatus::Created), "INITIALIZED");
        assert_eq!(webpay_status(PaymentStatus::Captured), "AUTHORIZED");
        assert_eq!(webpay_status(PaymentStatus::Rejected), "FAILED");
        assert_eq!(webpay_status(PaymentStatus::Refunded), "NULLIFIED");
        assert_eq!(webpay_status(PaymentStatus::Cancelled), "REVERSED");
    }
}
