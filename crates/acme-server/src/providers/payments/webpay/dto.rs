//! Trancorp Webpay's wire types (spec §10.2). Every request/response body
//! is a named `ToSchema` — the §22.3 rule `xtask spec-lint` enforces —
//! never an inline anonymous object.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request body for `POST .../transactions` (spec §10.2). No card data —
/// Webpay's real dialect never sees one either; the shopper enters it on
/// Transbank's own hosted page (here, `/webpay/checkout`).
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTransactionRequest {
    /// Merchant order reference, echoed back unchanged on commit.
    #[schema(example = "ORD-2026-000814")]
    pub buy_order: String,
    /// Merchant session identifier, echoed back unchanged on commit.
    #[schema(example = "sess-91ab")]
    pub session_id: String,
    /// Amount in the currency's minor units.
    #[schema(example = 45990)]
    pub amount: i64,
    /// Where the shopper's browser is redirected after the hosted page,
    /// as a form `POST` carrying `token_ws`.
    pub return_url: String,
}

/// Response body for `POST .../transactions`.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateTransactionResponse {
    /// Opaque token; the merchant redirects the shopper's browser to
    /// `url` with this as a `token_ws` form field.
    pub token: String,
    /// The hosted card-entry page (`/webpay/checkout`).
    pub url: String,
}

/// Masked card detail — only the last four digits, matching the real
/// dialect's own `card_number` field despite its name.
#[derive(Debug, Serialize, ToSchema)]
pub struct CardDetail {
    /// Last four digits only.
    pub card_number: String,
}

/// Response body shared by commit (`PUT .../transactions/{token}`) and
/// status (`GET .../transactions/{token}`) — spec §10.2's own worked
/// example commit response.
#[derive(Debug, Serialize, ToSchema)]
pub struct TransactionDetail {
    /// Card verification indicator. Always `TSY` in this sandbox.
    pub vci: String,
    /// Amount in the currency's minor units.
    pub amount: i64,
    /// `INITIALIZED`, `AUTHORIZED`, `FAILED`, `NULLIFIED`, or `REVERSED`.
    pub status: String,
    /// The merchant order reference from create.
    pub buy_order: String,
    /// The merchant session identifier from create.
    pub session_id: String,
    pub card_detail: CardDetail,
    /// `MMDD`, in simulated time.
    pub accounting_date: String,
    /// RFC3339 timestamp with millisecond precision, in simulated time.
    pub transaction_date: String,
    /// Issuer authorization code, present once authorized.
    pub authorization_code: Option<String>,
    /// Always `VN` (credit, no instalments) in this sandbox —
    /// `CreateTransactionRequest` carries no card/instalment data to
    /// derive spec §10.2's fuller `VD`/`VC`/`SI`/`S2`/`NC` set from.
    pub payment_type_code: String,
    /// `0` approved; `-1` rejected. See specs/006-Dialects.md's Caveats
    /// for why this sandbox trims spec §10.2's fuller response_code table.
    pub response_code: i32,
    /// Always `0` in this sandbox.
    pub installments_number: i32,
}

/// Request body for `POST .../transactions/{token}/refunds`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RefundRequest {
    /// Amount to refund, in minor units. Omit to refund the full amount
    /// (a full refund on an unrefunded balance nullifies the transaction).
    pub amount: Option<i64>,
}

/// Response body for a refund/nullify.
#[derive(Debug, Serialize, ToSchema)]
pub struct RefundResponse {
    /// `nullified` (full refund of a transaction with no prior refund) or
    /// `reversed` (partial, or a further refund on an already-refunded one).
    #[serde(rename = "type")]
    pub kind: String,
    /// Amount refunded by this call, in minor units.
    pub amount: i64,
    /// Always `0` — a refund the sandbox accepted always succeeds.
    pub response_code: i32,
}

/// Trancorp's own error envelope (spec §10.2/§10.6: `error_message`, not
/// the Stripe-shaped `AcmeErrorBody` the reference dialects use) — one of
/// this dialect's deliberately reproduced "awkward parts" (spec §20's
/// "Definition of done" item 2).
#[derive(Debug, Serialize, ToSchema)]
pub struct WebpayErrorBody {
    /// Human-readable explanation, e.g. `"Transaction already locked"`.
    pub error_message: String,
}
