//! Scenario / magic-value matching (spec §9): the highest-precedence match
//! (explicit override) always wins; magic values are the fallback so a
//! demo can force an outcome from any HTTP client without touching a
//! database. Fault-rule injection (spec §9.3) is dashboard-configured and
//! out of scope until M6.

use std::fmt;
use std::str::FromStr;

use chrono::Duration;
use serde::{Deserialize, Serialize};

use crate::domain::payment::{DeclineReason, PaymentCommand, PaymentStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentScenario {
    Approve,
    DeclineInsufficientFunds,
    DeclineExpiredCard,
    DeclineInvalidCvv,
    ProcessingError,
    ThreeDsChallenge,
    ThreeDsFail,
    Chargeback,
    SlowApproval,
    InvalidNumber,
    DeclineDoNotHonor,
    ManualReview,
    ProviderError,
    RiskReject,
    NeverPaid,
}

impl PaymentScenario {
    pub fn as_str(self) -> &'static str {
        use PaymentScenario::*;
        match self {
            Approve => "approve",
            DeclineInsufficientFunds => "decline_insufficient_funds",
            DeclineExpiredCard => "decline_expired_card",
            DeclineInvalidCvv => "decline_invalid_cvv",
            ProcessingError => "processing_error",
            ThreeDsChallenge => "three_ds_challenge",
            ThreeDsFail => "three_ds_fail",
            Chargeback => "chargeback",
            SlowApproval => "slow_approval",
            InvalidNumber => "invalid_number",
            DeclineDoNotHonor => "decline_do_not_honor",
            ManualReview => "manual_review",
            ProviderError => "provider_error",
            RiskReject => "risk_reject",
            NeverPaid => "never_paid",
        }
    }
}

impl fmt::Display for PaymentScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown scenario `{0}`")]
pub struct UnknownScenario(String);

impl FromStr for PaymentScenario {
    type Err = UnknownScenario;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use PaymentScenario::*;
        Ok(match s {
            "approve" => Approve,
            "decline_insufficient_funds" => DeclineInsufficientFunds,
            "decline_expired_card" => DeclineExpiredCard,
            "decline_invalid_cvv" => DeclineInvalidCvv,
            "processing_error" => ProcessingError,
            "three_ds_challenge" => ThreeDsChallenge,
            "three_ds_fail" => ThreeDsFail,
            "chargeback" => Chargeback,
            "slow_approval" => SlowApproval,
            "invalid_number" => InvalidNumber,
            "decline_do_not_honor" => DeclineDoNotHonor,
            "manual_review" => ManualReview,
            "provider_error" => ProviderError,
            "risk_reject" => RiskReject,
            "never_paid" => NeverPaid,
            other => return Err(UnknownScenario(other.to_string())),
        })
    }
}

/// Digits-only PAN, spec §9.1's published test numbers.
fn pan_scenario(pan: &str) -> Option<PaymentScenario> {
    let digits: String = pan.chars().filter(char::is_ascii_digit).collect();
    use PaymentScenario::*;
    Some(match digits.as_str() {
        "4111111111111111" => Approve,
        "5555555555554444" => Approve,
        "4000000000000002" => DeclineInsufficientFunds,
        "4000000000000069" => DeclineExpiredCard,
        "4000000000000127" => DeclineInvalidCvv,
        "4000000000000119" => ProcessingError,
        "4000000000003220" => ThreeDsChallenge,
        "4000000000003063" => ThreeDsFail,
        "4000000000000341" => Chargeback,
        "4000000000009995" => SlowApproval,
        "4242424242424241" => InvalidNumber,
        _ => return None,
    })
}

fn email_scenario(email: &str) -> Option<PaymentScenario> {
    match email {
        "decline@acme.test" => Some(PaymentScenario::DeclineDoNotHonor),
        "slow@acme.test" => Some(PaymentScenario::SlowApproval),
        "chargeback@acme.test" => Some(PaymentScenario::Chargeback),
        _ => None,
    }
}

fn amount_scenario(amount_cents: i64) -> Option<PaymentScenario> {
    let last_two = amount_cents.rem_euclid(100);
    if amount_cents > 5_000_000 {
        return Some(PaymentScenario::RiskReject);
    }
    match last_two {
        13 => Some(PaymentScenario::DeclineDoNotHonor),
        51 => Some(PaymentScenario::ManualReview),
        99 => Some(PaymentScenario::ProviderError),
        _ => None,
    }
}

/// Inputs a payment-create handler has on hand to pick a scenario (spec
/// §9.1). `method_kind` distinguishes the PIX/boleto-only `never_paid` rule
/// from the card-only PAN rules.
pub struct PaymentInputs<'a> {
    pub pan: Option<&'a str>,
    pub amount_cents: i64,
    pub email: Option<&'a str>,
    pub method_kind: &'a str,
    pub explicit: Option<&'a str>,
}

/// Resolves the scenario for a payment create request. Precedence (spec
/// §9, highest first): explicit override, then magic values, checked
/// PAN → email → amount → method-specific amount rule, defaulting to
/// `Approve`.
pub fn resolve_payment_scenario(
    input: &PaymentInputs<'_>,
) -> Result<PaymentScenario, UnknownScenario> {
    if let Some(explicit) = input.explicit {
        return PaymentScenario::from_str(explicit);
    }

    if let Some(pan) = input.pan
        && let Some(scenario) = pan_scenario(pan)
    {
        return Ok(scenario);
    }
    if let Some(email) = input.email
        && let Some(scenario) = email_scenario(email)
    {
        return Ok(scenario);
    }
    if matches!(input.method_kind, "pix" | "boleto") && input.amount_cents.rem_euclid(100) == 77 {
        return Ok(PaymentScenario::NeverPaid);
    }
    if let Some(scenario) = amount_scenario(input.amount_cents) {
        return Ok(scenario);
    }

    Ok(PaymentScenario::Approve)
}

/// A scenario resolved to `ProcessingError`, `ProviderError` or
/// `InvalidNumber` never becomes a `payments` row at all (spec §9.1: "HTTP
/// 502/500 from the provider", "400, fails Luhn") — the create-payment
/// handler returns the error directly instead of calling `initial_steps`.
pub fn payment_scenario_short_circuits(scenario: PaymentScenario) -> bool {
    matches!(
        scenario,
        PaymentScenario::ProcessingError
            | PaymentScenario::ProviderError
            | PaymentScenario::InvalidNumber
    )
}

/// The zero-delay commands applied synchronously at payment creation, from
/// `Created`. A scenario that ends up `Pending` may have a further delayed
/// step — see `payment_next_step` — driven by `sim::ticker`.
pub fn payment_initial_steps(scenario: PaymentScenario) -> Vec<PaymentCommand> {
    use PaymentCommand::*;
    use PaymentScenario::*;

    match scenario {
        Approve => vec![MarkPending, Authorize, Capture],
        DeclineInsufficientFunds => vec![Reject(DeclineReason::InsufficientFunds)],
        DeclineExpiredCard => vec![Reject(DeclineReason::CardExpired)],
        DeclineInvalidCvv => vec![Reject(DeclineReason::InvalidCvv)],
        DeclineDoNotHonor => vec![Reject(DeclineReason::DoNotHonor)],
        RiskReject => vec![Reject(DeclineReason::RiskRejected)],
        Chargeback => vec![MarkPending, Authorize, Capture],
        SlowApproval | ManualReview | ThreeDsChallenge | ThreeDsFail | NeverPaid => {
            vec![MarkPending]
        }
        ProcessingError | ProviderError | InvalidNumber => vec![],
    }
}

/// The next scheduled command for a scenario-driven payment sitting in
/// `current`, or `None` once the scenario has reached its resting state.
/// Mirrors `shipment::happy_path_schedule`'s role, but as a table lookup
/// rather than a precomputed vector — payment scenarios are at most two
/// hops deep, so there's nothing to gain from generating a schedule up
/// front.
pub fn payment_next_step(
    scenario: PaymentScenario,
    current: PaymentStatus,
) -> Option<(Duration, PaymentCommand)> {
    use PaymentCommand::*;
    use PaymentScenario::*;
    use PaymentStatus::*;

    match (scenario, current) {
        (SlowApproval, Pending) => Some((Duration::minutes(3), Authorize)),
        (SlowApproval, Authorized) => Some((Duration::zero(), Capture)),
        (ManualReview, Pending) => Some((Duration::minutes(10), Authorize)),
        (ManualReview, Authorized) => Some((Duration::zero(), Capture)),
        (ThreeDsChallenge, Pending) => Some((Duration::seconds(30), Authorize)),
        (ThreeDsChallenge, Authorized) => Some((Duration::zero(), Capture)),
        (ThreeDsFail, Pending) => {
            Some((Duration::seconds(30), Reject(DeclineReason::ThreeDsFailed)))
        }
        (Chargeback, Captured) => Some((Duration::days(2), OpenDispute)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShipmentScenario {
    Standard,
    NoCoverage,
    AddressException,
    RemoteArea,
    Oversized,
    RequiresInsurance,
    FailedAttempts,
    Lost,
    Delayed,
    Express,
}

impl ShipmentScenario {
    pub fn as_str(self) -> &'static str {
        use ShipmentScenario::*;
        match self {
            Standard => "standard",
            NoCoverage => "no_coverage",
            AddressException => "address_exception",
            RemoteArea => "remote_area",
            Oversized => "oversized",
            RequiresInsurance => "requires_insurance",
            FailedAttempts => "failed_attempts",
            Lost => "lost",
            Delayed => "delayed",
            Express => "express",
        }
    }
}

impl fmt::Display for ShipmentScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ShipmentScenario {
    type Err = UnknownScenario;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use ShipmentScenario::*;
        Ok(match s {
            "standard" => Standard,
            "no_coverage" => NoCoverage,
            "address_exception" => AddressException,
            "remote_area" => RemoteArea,
            "oversized" => Oversized,
            "requires_insurance" => RequiresInsurance,
            "failed_attempts" => FailedAttempts,
            "lost" => Lost,
            "delayed" => Delayed,
            "express" => Express,
            other => return Err(UnknownScenario(other.to_string())),
        })
    }
}

pub struct ShipmentPackageDims {
    pub weight_grams: i64,
    pub length_cm: i64,
    pub width_cm: i64,
    pub height_cm: i64,
}

pub struct ShipmentInputs<'a> {
    pub destination_postal_code: &'a str,
    pub destination_country: &'a str,
    pub packages: &'a [ShipmentPackageDims],
    pub declared_value_cents: i64,
    pub recipient_name: Option<&'a str>,
    pub order_reference: Option<&'a str>,
    pub explicit: Option<&'a str>,
}

/// Resolves the scenario for a rate/shipment-create request (spec §9.2).
/// Multiple rules can technically match the same request; the table order
/// below is also the precedence order among magic values, since it's
/// unspecified by the spec itself.
pub fn resolve_shipment_scenario(
    input: &ShipmentInputs<'_>,
) -> Result<ShipmentScenario, UnknownScenario> {
    if let Some(explicit) = input.explicit {
        return ShipmentScenario::from_str(explicit);
    }

    if input.destination_postal_code == "00000" {
        return Ok(ShipmentScenario::NoCoverage);
    }
    if input.destination_postal_code.starts_with("999") {
        return Ok(ShipmentScenario::AddressException);
    }
    if (input.destination_country == "ES" && input.destination_postal_code == "07001")
        || (input.destination_country == "CL" && input.destination_postal_code == "7550000")
    {
        return Ok(ShipmentScenario::RemoteArea);
    }
    if input.packages.iter().any(|p| {
        p.weight_grams > 30_000 || p.length_cm > 150 || p.width_cm > 150 || p.height_cm > 150
    }) {
        return Ok(ShipmentScenario::Oversized);
    }
    if input.declared_value_cents > 300_000 {
        return Ok(ShipmentScenario::RequiresInsurance);
    }
    if let Some(name) = input.recipient_name {
        let upper = name.to_uppercase();
        if upper.contains("NADIE") || upper.contains("NOBODY") {
            return Ok(ShipmentScenario::FailedAttempts);
        }
        if upper.contains("PERDIDO") || upper.contains("LOST") {
            return Ok(ShipmentScenario::Lost);
        }
    }
    if let Some(reference) = input.order_reference {
        let upper = reference.to_uppercase();
        if upper.contains("SLOWSHIP") {
            return Ok(ShipmentScenario::Delayed);
        }
        if upper.contains("FASTSHIP") {
            return Ok(ShipmentScenario::Express);
        }
    }

    Ok(ShipmentScenario::Standard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::clock::SimClock;
    use chrono::Utc;

    #[test]
    fn every_documented_payment_pan_matches_its_scenario() {
        let cases = [
            ("4111111111111111", PaymentScenario::Approve),
            ("5555555555554444", PaymentScenario::Approve),
            (
                "4000000000000002",
                PaymentScenario::DeclineInsufficientFunds,
            ),
            ("4000000000000069", PaymentScenario::DeclineExpiredCard),
            ("4000000000000127", PaymentScenario::DeclineInvalidCvv),
            ("4000000000000119", PaymentScenario::ProcessingError),
            ("4000000000003220", PaymentScenario::ThreeDsChallenge),
            ("4000000000003063", PaymentScenario::ThreeDsFail),
            ("4000000000000341", PaymentScenario::Chargeback),
            ("4000000000009995", PaymentScenario::SlowApproval),
            ("4242424242424241", PaymentScenario::InvalidNumber),
        ];
        for (pan, expected) in cases {
            let resolved = resolve_payment_scenario(&PaymentInputs {
                pan: Some(pan),
                amount_cents: 10_000,
                email: None,
                method_kind: "card",
                explicit: None,
            })
            .unwrap();
            assert_eq!(resolved, expected, "pan {pan}");
        }
    }

    #[test]
    fn amount_suffix_rules_match() {
        assert_eq!(
            resolve_payment_scenario(&PaymentInputs {
                pan: None,
                amount_cents: 1013,
                email: None,
                method_kind: "card",
                explicit: None
            })
            .unwrap(),
            PaymentScenario::DeclineDoNotHonor
        );
        assert_eq!(
            resolve_payment_scenario(&PaymentInputs {
                pan: None,
                amount_cents: 1051,
                email: None,
                method_kind: "card",
                explicit: None
            })
            .unwrap(),
            PaymentScenario::ManualReview
        );
        assert_eq!(
            resolve_payment_scenario(&PaymentInputs {
                pan: None,
                amount_cents: 1099,
                email: None,
                method_kind: "card",
                explicit: None
            })
            .unwrap(),
            PaymentScenario::ProviderError
        );
        assert_eq!(
            resolve_payment_scenario(&PaymentInputs {
                pan: None,
                amount_cents: 5_000_001,
                email: None,
                method_kind: "card",
                explicit: None
            })
            .unwrap(),
            PaymentScenario::RiskReject
        );
    }

    #[test]
    fn email_rules_match() {
        assert_eq!(
            resolve_payment_scenario(&PaymentInputs {
                pan: None,
                amount_cents: 100,
                email: Some("decline@acme.test"),
                method_kind: "card",
                explicit: None
            })
            .unwrap(),
            PaymentScenario::DeclineDoNotHonor
        );
    }

    #[test]
    fn pix_amount_ending_77_never_paid() {
        assert_eq!(
            resolve_payment_scenario(&PaymentInputs {
                pan: None,
                amount_cents: 1077,
                email: None,
                method_kind: "pix",
                explicit: None
            })
            .unwrap(),
            PaymentScenario::NeverPaid
        );
    }

    #[test]
    fn no_magic_value_defaults_to_approve() {
        assert_eq!(
            resolve_payment_scenario(&PaymentInputs {
                pan: Some("4000000000000000"),
                amount_cents: 100,
                email: None,
                method_kind: "card",
                explicit: None
            })
            .unwrap(),
            PaymentScenario::Approve
        );
    }

    #[test]
    fn explicit_override_always_wins() {
        let resolved = resolve_payment_scenario(&PaymentInputs {
            pan: Some("4000000000000002"), // would otherwise decline
            amount_cents: 100,
            email: None,
            method_kind: "card",
            explicit: Some("approve"),
        })
        .unwrap();
        assert_eq!(resolved, PaymentScenario::Approve);
    }

    #[test]
    fn unknown_explicit_scenario_errors() {
        assert!(
            resolve_payment_scenario(&PaymentInputs {
                pan: None,
                amount_cents: 100,
                email: None,
                method_kind: "card",
                explicit: Some("not_a_real_scenario"),
            })
            .is_err()
        );
    }

    fn dims(weight_grams: i64, l: i64, w: i64, h: i64) -> ShipmentPackageDims {
        ShipmentPackageDims {
            weight_grams,
            length_cm: l,
            width_cm: w,
            height_cm: h,
        }
    }

    #[test]
    fn every_documented_shipment_magic_value_matches() {
        let base_pkg = [dims(1000, 10, 10, 10)];
        let oversized_pkg = [dims(31_000, 10, 10, 10)];
        let cases: Vec<(ShipmentInputs, ShipmentScenario)> = vec![
            (
                ShipmentInputs {
                    destination_postal_code: "00000",
                    destination_country: "ES",
                    packages: &base_pkg,
                    declared_value_cents: 0,
                    recipient_name: None,
                    order_reference: None,
                    explicit: None,
                },
                ShipmentScenario::NoCoverage,
            ),
            (
                ShipmentInputs {
                    destination_postal_code: "99901",
                    destination_country: "CL",
                    packages: &base_pkg,
                    declared_value_cents: 0,
                    recipient_name: None,
                    order_reference: None,
                    explicit: None,
                },
                ShipmentScenario::AddressException,
            ),
            (
                ShipmentInputs {
                    destination_postal_code: "07001",
                    destination_country: "ES",
                    packages: &base_pkg,
                    declared_value_cents: 0,
                    recipient_name: None,
                    order_reference: None,
                    explicit: None,
                },
                ShipmentScenario::RemoteArea,
            ),
            (
                ShipmentInputs {
                    destination_postal_code: "28001",
                    destination_country: "ES",
                    packages: &oversized_pkg,
                    declared_value_cents: 0,
                    recipient_name: None,
                    order_reference: None,
                    explicit: None,
                },
                ShipmentScenario::Oversized,
            ),
            (
                ShipmentInputs {
                    destination_postal_code: "28001",
                    destination_country: "ES",
                    packages: &base_pkg,
                    declared_value_cents: 300_001,
                    recipient_name: None,
                    order_reference: None,
                    explicit: None,
                },
                ShipmentScenario::RequiresInsurance,
            ),
            (
                ShipmentInputs {
                    destination_postal_code: "28001",
                    destination_country: "ES",
                    packages: &base_pkg,
                    declared_value_cents: 0,
                    recipient_name: Some("NADIE EN CASA"),
                    order_reference: None,
                    explicit: None,
                },
                ShipmentScenario::FailedAttempts,
            ),
            (
                ShipmentInputs {
                    destination_postal_code: "28001",
                    destination_country: "ES",
                    packages: &base_pkg,
                    declared_value_cents: 0,
                    recipient_name: Some("PERDIDO"),
                    order_reference: None,
                    explicit: None,
                },
                ShipmentScenario::Lost,
            ),
            (
                ShipmentInputs {
                    destination_postal_code: "28001",
                    destination_country: "ES",
                    packages: &base_pkg,
                    declared_value_cents: 0,
                    recipient_name: None,
                    order_reference: Some("ORD-SLOWSHIP-1"),
                    explicit: None,
                },
                ShipmentScenario::Delayed,
            ),
            (
                ShipmentInputs {
                    destination_postal_code: "28001",
                    destination_country: "ES",
                    packages: &base_pkg,
                    declared_value_cents: 0,
                    recipient_name: None,
                    order_reference: Some("ORD-FASTSHIP-1"),
                    explicit: None,
                },
                ShipmentScenario::Express,
            ),
            (
                ShipmentInputs {
                    destination_postal_code: "28001",
                    destination_country: "ES",
                    packages: &base_pkg,
                    declared_value_cents: 0,
                    recipient_name: None,
                    order_reference: None,
                    explicit: None,
                },
                ShipmentScenario::Standard,
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(resolve_shipment_scenario(&input).unwrap(), expected);
        }
    }

    #[test]
    fn approve_steps_reach_captured_synchronously() {
        let mut status = PaymentStatus::Created;
        let clock = SimClock::new(Utc::now(), 60.0);
        for cmd in payment_initial_steps(PaymentScenario::Approve) {
            status = crate::domain::payment::apply(status, cmd, &clock)
                .unwrap()
                .to;
        }
        assert_eq!(status, PaymentStatus::Captured);
    }

    #[test]
    fn slow_approval_schedules_a_delayed_authorize_then_capture() {
        let (delay, cmd) =
            payment_next_step(PaymentScenario::SlowApproval, PaymentStatus::Pending).unwrap();
        assert_eq!(delay, chrono::Duration::minutes(3));
        assert_eq!(cmd, PaymentCommand::Authorize);
        let (delay, cmd) =
            payment_next_step(PaymentScenario::SlowApproval, PaymentStatus::Authorized).unwrap();
        assert_eq!(delay, chrono::Duration::zero());
        assert_eq!(cmd, PaymentCommand::Capture);
        assert!(
            payment_next_step(PaymentScenario::SlowApproval, PaymentStatus::Captured).is_none()
        );
    }

    #[test]
    fn short_circuit_scenarios_never_produce_initial_steps() {
        for scenario in [
            PaymentScenario::ProcessingError,
            PaymentScenario::ProviderError,
            PaymentScenario::InvalidNumber,
        ] {
            assert!(payment_scenario_short_circuits(scenario));
            assert!(payment_initial_steps(scenario).is_empty());
        }
    }

    #[test]
    fn explicit_shipment_override_always_wins() {
        let resolved = resolve_shipment_scenario(&ShipmentInputs {
            destination_postal_code: "00000", // would otherwise be no_coverage
            destination_country: "ES",
            packages: &[dims(1000, 10, 10, 10)],
            declared_value_cents: 0,
            recipient_name: None,
            order_reference: None,
            explicit: Some("standard"),
        })
        .unwrap();
        assert_eq!(resolved, ShipmentScenario::Standard);
    }
}
