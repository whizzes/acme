//! Maps between `db::repo::payments` rows and Acme Pay's wire DTOs, and the
//! handful of card-detail helpers a real card network would provide.

use serde::{Deserialize, Serialize};

use crate::db::repo::payments::PaymentRow;
use crate::providers::payments::acmepay::dto::{
    CardOutput, Payment, PaymentMethodOutput, RiskInfo,
};

/// Card facts derived at creation time and persisted as JSON in
/// `payments.method_detail` — never the PAN itself (spec §18).
#[derive(Debug, Serialize, Deserialize)]
pub struct StoredCardDetail {
    pub brand: String,
    pub last4: String,
    pub exp_month: i32,
    pub exp_year: i32,
    pub token: String,
    pub installments: i32,
}

/// Visa/Mastercard by leading digit — the only two brands spec §9.1's test
/// PANs use.
pub fn brand_from_pan(pan: &str) -> &'static str {
    match pan.chars().next() {
        Some('4') => "visa",
        Some('5') => "mastercard",
        _ => "unknown",
    }
}

pub fn last4(pan: &str) -> String {
    let digits: String = pan.chars().filter(char::is_ascii_digit).collect();
    let len = digits.len();
    digits[len.saturating_sub(4)..].to_string()
}

pub fn payment_to_dto(row: &PaymentRow) -> Payment {
    let stored_card: Option<StoredCardDetail> = row
        .method_detail
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    let payment_method = PaymentMethodOutput {
        kind: row.method_kind.clone(),
        card: stored_card.map(|d| CardOutput {
            brand: d.brand,
            last4: d.last4,
            exp_month: d.exp_month,
            exp_year: d.exp_year,
            token: d.token,
            installments: d.installments,
        }),
    };

    let risk = row.risk_decision.clone().map(|decision| RiskInfo {
        score: row.risk_score.unwrap_or(0),
        decision,
    });

    Payment {
        id: row.id.to_string(),
        object: "payment".to_string(),
        status: row.status,
        status_reason: row.status_reason.clone(),
        amount: row.amount_cents,
        amount_refunded: row.amount_refunded_cents,
        currency: row.currency.clone(),
        reference: row.reference.clone(),
        capture_mode: row.capture_mode.clone(),
        payment_method,
        authorization_code: row.auth_code.clone(),
        risk,
        livemode: false,
        created_at: row.created_at.to_rfc3339(),
        captured_at: row.captured_at.map(|t| t.to_rfc3339()),
        metadata: row
            .metadata
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_detection_matches_test_pan_prefixes() {
        assert_eq!(brand_from_pan("4111111111111111"), "visa");
        assert_eq!(brand_from_pan("5555555555554444"), "mastercard");
    }

    #[test]
    fn last4_keeps_final_digits_only() {
        assert_eq!(last4("4111 1111 1111 1111"), "1111");
    }
}
