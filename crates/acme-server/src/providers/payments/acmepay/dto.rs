//! Acme Pay's wire types (spec §10.1). Every request/response body is a
//! named `ToSchema` — the §22.3 rule `xtask spec-lint` enforces — never an
//! inline anonymous object.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// A card to charge. Only the published test PANs (spec §9.1, §18) are
/// ever accepted; the full number is never persisted.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CardInput {
    /// Card number. Only spec §9.1's published test PANs are accepted.
    #[schema(example = "4111111111111111")]
    pub number: String,
    /// Expiry month, 1-12.
    #[schema(example = 11)]
    pub exp_month: i32,
    /// Expiry year, four digits.
    #[schema(example = 2029)]
    pub exp_year: i32,
    /// Card verification value. Never persisted or echoed back.
    pub cvv: Option<String>,
    /// Cardholder name as printed on the card.
    pub holder: Option<String>,
}

/// The payment instrument for a create-payment request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PaymentMethodInput {
    /// Instrument kind: `card`, `pix`, or `boleto`.
    #[serde(rename = "type")]
    #[schema(example = "card")]
    pub kind: String,
    /// Card details, required when `type` is `card`.
    pub card: Option<CardInput>,
    /// Number of instalments, when the method supports them.
    pub installments: Option<i32>,
}

/// The paying customer's identity, for receipts and risk scoring.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CustomerInput {
    /// Customer's full name.
    pub name: Option<String>,
    /// Customer's email — also checked against spec §9.1's magic-value
    /// email rules (e.g. `decline@acme.test`).
    #[schema(example = "camila@example.cl")]
    pub email: Option<String>,
    /// Local identity document type, e.g. `RUT`.
    pub doc_type: Option<String>,
    /// Local identity document number.
    pub doc_number: Option<String>,
}

/// Request body for `POST /payments` (spec §10.1).
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreatePaymentRequest {
    /// Amount in the currency's minor units (spec Appendix C — CLP has
    /// zero minor units, so this is whole pesos for CLP).
    #[schema(example = 4599000)]
    pub amount: i64,
    /// ISO 4217 currency code.
    #[schema(example = "CLP")]
    pub currency: String,
    /// `automatic` (default) captures immediately on approval; `manual`
    /// leaves the payment `authorized` until `POST .../capture`.
    pub capture_mode: Option<String>,
    /// Merchant-supplied order reference, echoed back unchanged.
    pub reference: Option<String>,
    /// Free-text description, shown in the dashboard only.
    pub description: Option<String>,
    /// The paying customer, for receipts and risk scoring.
    pub customer: Option<CustomerInput>,
    /// The instrument to charge.
    pub payment_method: PaymentMethodInput,
    /// Arbitrary merchant metadata, echoed back unchanged. Also where the
    /// `acme_scenario` explicit override (spec §9) is read from.
    #[schema(value_type = Option<Object>)]
    pub metadata: Option<serde_json::Value>,
}

/// A tokenized card, safe to echo back — never the full PAN.
#[derive(Debug, Serialize, ToSchema)]
pub struct CardOutput {
    /// Card network, inferred from the PAN's leading digits.
    pub brand: String,
    /// Last four digits of the card number.
    pub last4: String,
    /// Expiry month, 1-12.
    pub exp_month: i32,
    /// Expiry year, four digits.
    pub exp_year: i32,
    /// Opaque token standing in for the full card number.
    pub token: String,
    /// Number of instalments this charge was split into.
    pub installments: i32,
}

/// The payment instrument as returned on a `Payment`.
#[derive(Debug, Serialize, ToSchema)]
pub struct PaymentMethodOutput {
    /// Instrument kind: `card`, `pix`, or `boleto`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Card details, present when `type` is `card`.
    pub card: Option<CardOutput>,
}

/// Simulated risk-engine verdict.
#[derive(Debug, Serialize, ToSchema)]
pub struct RiskInfo {
    /// 0 (lowest risk) to 100 (highest).
    pub score: i32,
    /// `approve`, `review`, or `reject`.
    pub decision: String,
}

/// A payment resource (spec §10.1).
#[derive(Debug, Serialize, ToSchema)]
pub struct Payment {
    /// Prefixed ULID, e.g. `pay_01J…`.
    pub id: String,
    /// Always `"payment"`.
    pub object: String,
    pub status: crate::domain::payment::PaymentStatus,
    /// Why a `rejected`/`cancelled` payment ended there, when known.
    pub status_reason: Option<String>,
    /// Amount in the currency's minor units.
    pub amount: i64,
    /// Total refunded so far, in the currency's minor units.
    pub amount_refunded: i64,
    /// ISO 4217 currency code.
    pub currency: String,
    /// Merchant-supplied order reference, echoed back unchanged.
    pub reference: Option<String>,
    /// `automatic` or `manual` — see `CreatePaymentRequest.capture_mode`.
    pub capture_mode: String,
    pub payment_method: PaymentMethodOutput,
    /// Issuer authorization code, present once authorized.
    pub authorization_code: Option<String>,
    pub risk: Option<RiskInfo>,
    /// Always `false` — this is a sandbox; no real money ever moves.
    pub livemode: bool,
    /// RFC3339 creation timestamp, in simulated time.
    pub created_at: String,
    /// RFC3339 capture timestamp, present once captured.
    pub captured_at: Option<String>,
    /// Arbitrary merchant metadata, echoed back unchanged.
    #[schema(value_type = Option<Object>)]
    pub metadata: Option<serde_json::Value>,
}

/// A cursor-paginated list (spec §10.1's Stripe-style idiom).
#[derive(Debug, Serialize, ToSchema)]
pub struct PaymentList {
    /// Always `"list"`.
    pub object: String,
    /// The endpoint this list was fetched from.
    pub url: String,
    /// Whether a further page exists.
    pub has_more: bool,
    /// Payments in this page, newest first.
    pub data: Vec<Payment>,
    /// Pass as `starting_after` to fetch the next page.
    pub next_cursor: Option<String>,
}

/// Query parameters for `GET /payments`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListPaymentsQuery {
    /// Filter by exact lifecycle status, e.g. `captured`.
    pub status: Option<String>,
    /// Only payments created strictly after this RFC3339 timestamp.
    pub created_after: Option<String>,
    /// Max rows to return, default 10.
    pub limit: Option<i64>,
    /// Cursor from a previous page's `next_cursor`.
    pub starting_after: Option<String>,
}

/// Request body for `POST /payments/{id}/capture`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CaptureRequest {
    /// Amount to capture, in minor units. Omit to capture the full
    /// authorized amount.
    pub amount: Option<i64>,
}

/// Request body for `POST /payments/{id}/refunds`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RefundRequest {
    /// Amount to refund, in minor units. Omit to refund the full amount.
    pub amount: Option<i64>,
    /// Free-text reason, shown in the dashboard.
    pub reason: Option<String>,
}

/// A refund resource.
#[derive(Debug, Serialize, ToSchema)]
pub struct Refund {
    /// Prefixed ULID, e.g. `ref_01J…`.
    pub id: String,
    /// The payment this refund was issued against.
    pub payment_id: String,
    /// Amount refunded, in minor units.
    pub amount: i64,
    /// Free-text reason, shown in the dashboard.
    pub reason: Option<String>,
    /// Always `succeeded` — refunds settle synchronously in this sandbox.
    pub status: String,
    /// RFC3339 creation timestamp, in simulated time.
    pub created_at: String,
}

/// List of refunds for one payment.
#[derive(Debug, Serialize, ToSchema)]
pub struct RefundList {
    /// Always `"list"`.
    pub object: String,
    /// Refunds for this payment, oldest first.
    pub data: Vec<Refund>,
}

/// Request body for `POST /checkout/sessions`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateCheckoutSessionRequest {
    /// Amount in the currency's minor units.
    pub amount: i64,
    /// ISO 4217 currency code.
    pub currency: String,
    /// Opaque line-item data, echoed back unchanged.
    #[schema(value_type = Option<Object>)]
    pub line_items: Option<serde_json::Value>,
    /// Shopper's email, pre-filled on the hosted page.
    pub customer_email: Option<String>,
    /// Where the shopper is redirected after a successful payment.
    pub success_url: Option<String>,
    /// Where the shopper is redirected after cancelling.
    pub cancel_url: Option<String>,
}

/// A hosted checkout session and its associated payment, when available.
#[derive(Debug, Serialize, ToSchema)]
pub struct CheckoutSession {
    /// Prefixed ULID, e.g. `cs_01J…`.
    pub id: String,
    /// Always `"checkout_session"`.
    pub object: String,
    /// `open`, `closed`, or `expired`.
    pub status: String,
    /// `unpaid`, or `paid`.
    pub payment_status: String,
    /// Associated payment ID, or null before a payment is created.
    pub payment_id: Option<String>,
    /// Amount in the currency's minor units.
    pub amount: i64,
    /// ISO 4217 currency code.
    pub currency: String,
    /// URL the merchant redirects the shopper to.
    pub hosted_url: String,
    /// RFC3339 expiry timestamp.
    pub expires_at: String,
    /// RFC3339 creation timestamp.
    pub created_at: String,
}

/// One row of the merchant's event log (spec §10.1 `GET /events`).
#[derive(Debug, Serialize, ToSchema)]
pub struct EventLogEntry {
    /// Prefixed ULID, e.g. `evt_01J…`.
    pub id: String,
    /// Canonical dot-separated event type, e.g. `payment.captured`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The kind of resource this event concerns, e.g. `payment`.
    pub resource_type: String,
    /// The id of the resource this event concerns.
    pub resource_id: String,
    /// Event-specific payload.
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
    /// RFC3339 timestamp the event occurred at.
    pub created_at: String,
}

/// Cursor-paginated event log.
#[derive(Debug, Serialize, ToSchema)]
pub struct EventList {
    /// Always `"list"`.
    pub object: String,
    /// Events in this page, newest first.
    pub data: Vec<EventLogEntry>,
    /// Pass as `starting_after` to fetch the next page.
    pub next_cursor: Option<String>,
}

/// Query parameters for `GET /events`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListEventsQuery {
    /// Max rows to return, default 10.
    pub limit: Option<i64>,
    /// Cursor from a previous page's `next_cursor`.
    pub starting_after: Option<String>,
}

/// Request body for `POST /webhook_endpoints`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateWebhookEndpointRequest {
    /// URL `acme` will `POST` event payloads to.
    pub url: String,
    /// Event types to deliver, e.g. `["payment.captured"]`. Empty means all.
    pub enabled_events: Vec<String>,
    /// Free-text note, shown in the dashboard.
    pub description: Option<String>,
}

/// A registered webhook endpoint. Delivery (signing, retries, the
/// dispatcher) is `dashboard::dispatcher`, spec §12.
#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookEndpoint {
    /// Prefixed ULID, e.g. `whe_01J…`.
    pub id: String,
    /// URL `acme` will `POST` event payloads to.
    pub url: String,
    /// Event types to deliver. Empty means all.
    pub enabled_events: Vec<String>,
    /// Whether this endpoint currently receives deliveries.
    pub active: bool,
    /// Free-text note, shown in the dashboard.
    pub description: Option<String>,
    /// RFC3339 creation timestamp.
    pub created_at: String,
}

/// One entry of the payment-methods catalog (spec §10.1 `GET
/// /payment_methods`).
#[derive(Debug, Serialize, ToSchema)]
pub struct PaymentMethodCatalogEntry {
    /// `card`, `pix`, or `boleto`.
    pub kind: String,
    /// Human-readable label, shown in the dashboard.
    pub label: String,
    /// ISO 3166-1 alpha-2 countries this method is offered in.
    pub countries: Vec<String>,
    /// ISO 4217 currencies this method supports.
    pub currencies: Vec<String>,
}

/// Payment methods catalog response.
#[derive(Debug, Serialize, ToSchema)]
pub struct PaymentMethodsResponse {
    /// Supported payment methods.
    pub data: Vec<PaymentMethodCatalogEntry>,
}
