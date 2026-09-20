//! Acme Ship's wire types (spec §11.1). Every request/response body is a
//! named `ToSchema` — the §22.3 rule `xtask spec-lint` enforces — never an
//! inline anonymous object.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// A postal address for rating/shipping (spec §11.1).
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AddressInput {
    /// Destination or origin postal code.
    pub postal_code: String,
    /// City name, for display only.
    pub city: Option<String>,
    /// ISO 3166-1 alpha-2 country code.
    #[schema(example = "ES")]
    pub country: String,
    /// Whether this is a home address — affects the residential surcharge.
    pub residential: Option<bool>,
}

/// One physical parcel.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct PackageInput {
    /// Actual weight, in grams.
    pub weight_grams: i64,
    /// Length, in centimeters.
    pub length_cm: i64,
    /// Width, in centimeters.
    pub width_cm: i64,
    /// Height, in centimeters.
    pub height_cm: i64,
}

/// An amount with its currency, for request bodies.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct MoneyInput {
    /// Amount in the currency's minor units.
    pub amount: i64,
    /// ISO 4217 currency code.
    pub currency: String,
}

/// Optional rating add-ons (spec §11.1).
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct RateOptionsInput {
    /// Add compulsory insurance (0.9% of declared value, minimum charge).
    pub insurance: Option<bool>,
    /// Collect payment from the recipient on delivery.
    pub cash_on_delivery: Option<bool>,
    /// Deliver on Saturday, for a fixed surcharge.
    pub saturday_delivery: Option<bool>,
}

/// Request body for `POST /rates`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RateRequest {
    pub origin: AddressInput,
    pub destination: AddressInput,
    /// Parcels to ship together; billed on their combined billable weight.
    pub packages: Vec<PackageInput>,
    pub declared_value: MoneyInput,
    pub options: Option<RateOptionsInput>,
}

/// One priced surcharge line.
#[derive(Debug, Serialize, ToSchema)]
pub struct SurchargeOutput {
    /// Stable machine-readable surcharge code, e.g. `fuel`.
    pub code: String,
    /// Human-readable label, shown in the dashboard.
    pub label: String,
    /// Amount in the currency's minor units.
    pub amount: i64,
}

/// The price breakdown backing `amount`.
#[derive(Debug, Serialize, ToSchema)]
pub struct PriceBreakdown {
    /// Base price before surcharges, in the currency's minor units.
    pub base: i64,
    /// Each priced surcharge line.
    pub surcharges: Vec<SurchargeOutput>,
}

/// Estimated transit window.
#[derive(Debug, Serialize, ToSchema)]
pub struct EstimatedDelivery {
    /// Fastest plausible transit time, in days.
    pub min_days: i32,
    /// Slowest plausible transit time, in days.
    pub max_days: i32,
    /// RFC3339 estimated delivery timestamp.
    pub eta: String,
}

/// An amount with its currency, for response bodies.
#[derive(Debug, Serialize, ToSchema)]
pub struct MoneyOutput {
    /// Amount in the currency's minor units.
    pub value: i64,
    /// ISO 4217 currency code.
    pub currency: String,
}

/// One rated shipping option (spec §11.1).
#[derive(Debug, Serialize, ToSchema)]
pub struct RateOption {
    /// Prefixed ULID, e.g. `rto_01J…`. Pass to `POST /shipments` to book it.
    pub id: String,
    /// Always `"acmeship"` — this dialect's own carrier code.
    pub carrier: String,
    /// `standard` or `express_24h`.
    pub service_code: String,
    /// Human-readable service name, shown in the dashboard.
    pub service_name: String,
    pub amount: MoneyOutput,
    pub breakdown: PriceBreakdown,
    /// The greater of actual and volumetric weight, in grams.
    pub billable_weight_grams: i64,
    pub estimated_delivery: EstimatedDelivery,
    /// Simulated carbon estimate, grams.
    pub co2_grams: i64,
}

/// Response body for `POST /rates`.
#[derive(Debug, Serialize, ToSchema)]
pub struct RateQuoteResponse {
    /// Prefixed ULID, e.g. `qte_01J…`.
    pub quote_id: String,
    /// Options are valid until this RFC3339 timestamp.
    pub expires_at: String,
    /// Priced options, empty when the destination has no coverage.
    pub options: Vec<RateOption>,
}

/// Request body for `POST /shipments`. Provide `rate_option_id` to create
/// from a prior `/rates` quote, or `origin`/`destination`/`packages`
/// directly for a one-shot booking.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateShipmentRequest {
    /// A `RateOption.id` from a prior `POST /rates` call.
    pub rate_option_id: Option<String>,
    /// Required without `rate_option_id`.
    pub origin: Option<AddressInput>,
    /// Required without `rate_option_id`.
    pub destination: Option<AddressInput>,
    /// Required without `rate_option_id`.
    pub packages: Option<Vec<PackageInput>>,
    /// Required without `rate_option_id`.
    pub declared_value: Option<MoneyInput>,
    /// Required when creating without a `rate_option_id`.
    pub service_code: Option<String>,
    /// Merchant-supplied order reference — also checked against spec
    /// §9.2's `SLOWSHIP`/`FASTSHIP` magic values.
    pub order_reference: Option<String>,
    /// Recipient's name — also checked against spec §9.2's
    /// `NADIE`/`NOBODY`/`PERDIDO`/`LOST` magic values.
    pub recipient_name: Option<String>,
    /// Arbitrary merchant metadata. Also where the `acme_scenario`
    /// explicit override (spec §9) is read from.
    #[schema(value_type = Option<Object>)]
    pub metadata: Option<serde_json::Value>,
}

/// A shipment resource (spec §11.1).
#[derive(Debug, Serialize, ToSchema)]
pub struct Shipment {
    /// Prefixed ULID, e.g. `shp_01J…`.
    pub id: String,
    /// Always `"shipment"`.
    pub object: String,
    pub status: crate::domain::shipment::ShipmentStatus,
    /// Carrier tracking number.
    pub tracking_number: String,
    /// Always `"acmeship"` — this dialect's own carrier code.
    pub carrier: String,
    /// `standard` or `express_24h`.
    pub service_code: String,
    pub price: MoneyOutput,
    /// RFC3339 estimated delivery timestamp.
    pub eta: Option<String>,
    /// Set once a label has been generated (lazily, on first `GET
    /// .../label`).
    pub label_url: Option<String>,
    /// Set once a return label has been generated for this shipment.
    pub return_tracking_number: Option<String>,
    /// RFC3339 creation timestamp, in simulated time.
    pub created_at: String,
    /// RFC3339 timestamp of the shipment's last status change.
    pub updated_at: String,
}

/// Cursor-paginated list of shipments.
#[derive(Debug, Serialize, ToSchema)]
pub struct ShipmentList {
    /// Always `"list"`.
    pub object: String,
    /// Whether a further page exists.
    pub has_more: bool,
    /// Shipments in this page, newest first.
    pub data: Vec<Shipment>,
    /// Pass as `starting_after` to fetch the next page.
    pub next_cursor: Option<String>,
}

/// Query parameters for `GET /shipments`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListShipmentsQuery {
    /// Filter by exact lifecycle status, e.g. `delivered`.
    pub status: Option<String>,
    /// Max rows to return, default 10.
    pub limit: Option<i64>,
    /// Cursor from a previous page's `next_cursor`.
    pub starting_after: Option<String>,
}

/// Query parameters for `GET /shipments/{id}/label`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LabelQuery {
    /// `pdf` (default), `zpl`, or `png`.
    pub format: Option<String>,
}

/// A generated shipping label.
#[derive(Debug, Serialize, ToSchema)]
pub struct LabelResponse {
    /// The shipment this label was generated for.
    pub shipment_id: String,
    /// `pdf`, `zpl`, or `png`.
    pub format: String,
    /// Where the (synthetic) label content lives.
    pub url: String,
}

/// One normalized tracking event.
#[derive(Debug, Serialize, ToSchema)]
pub struct TrackingEvent {
    pub status: crate::domain::shipment::ShipmentStatus,
    /// Human-readable description of this event.
    pub description: String,
    /// RFC3339 timestamp the event occurred at, in simulated time.
    pub occurred_at: String,
}

/// Normalized tracking history for one shipment (spec §11.1's `GET
/// /shipments/{id}/tracking` and the public `GET /tracking/{tracking_number}`
/// share this shape).
#[derive(Debug, Serialize, ToSchema)]
pub struct TrackingResponse {
    /// Carrier tracking number.
    pub tracking_number: String,
    /// The shipment's current status — also the last event's status.
    pub status: crate::domain::shipment::ShipmentStatus,
    /// Every recorded event, oldest first.
    pub events: Vec<TrackingEvent>,
}

/// Request body for `POST /pickups`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreatePickupRequest {
    pub address: AddressInput,
    /// RFC3339 window start.
    pub window_start: String,
    /// RFC3339 window end.
    pub window_end: String,
    /// Shipment ids to collect.
    pub shipment_ids: Vec<String>,
}

/// A scheduled collection request.
#[derive(Debug, Serialize, ToSchema)]
pub struct Pickup {
    /// Prefixed ULID, e.g. `pck_01J…`.
    pub id: String,
    /// Always `"scheduled"` — pickups don't change state in this
    /// milestone.
    pub status: String,
    /// Carrier confirmation code for the driver.
    pub confirmation_code: Option<String>,
    /// RFC3339 window start.
    pub window_start: String,
    /// RFC3339 window end.
    pub window_end: String,
    /// RFC3339 creation timestamp.
    pub created_at: String,
}

/// Request body for `POST /returns`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateReturnRequest {
    /// The outbound shipment to generate a return label for.
    pub shipment_id: String,
}

/// A generated return label.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReturnResponse {
    /// The outbound shipment this return label was generated for.
    pub shipment_id: String,
    /// New tracking number for the return leg.
    pub return_tracking_number: String,
    /// Where the (synthetic) return label content lives.
    pub label_url: String,
}

/// Query parameters for `GET /coverage`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CoverageQuery {
    /// ISO 3166-1 alpha-2 country code.
    pub country: String,
    /// Destination postal code to check.
    pub postal_code: String,
}

/// Serviceability for one destination.
#[derive(Debug, Serialize, ToSchema)]
pub struct CoverageResponse {
    /// Whether this provider delivers to the destination at all.
    pub supported: bool,
    /// Service codes available for this destination.
    pub services: Vec<String>,
    /// Extra transit days for a remote area, if any.
    pub extra_days: i32,
    /// Extra surcharge for a remote area, in minor units.
    pub surcharge_cents: i64,
}

/// One entry of the service catalog.
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceCatalogEntry {
    /// `standard` or `express_24h`.
    pub code: String,
    /// Human-readable name, shown in the dashboard.
    pub name: String,
    /// Fastest plausible transit time, in days.
    pub eta_min_days: i32,
    /// Slowest plausible transit time, in days.
    pub eta_max_days: i32,
}

/// Response body for `GET /services`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ServicesResponse {
    /// Every service level this provider offers.
    pub data: Vec<ServiceCatalogEntry>,
}
