//! Iberex Express's wire types (spec §11.2). Every request/response body
//! is a named `ToSchema` — the §22.3 rule `xtask spec-lint` enforces —
//! never an inline anonymous object.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request body for `POST /services/oauth2/token` (`grant_type=password`,
/// form-encoded per the OAuth2 password grant).
#[derive(Debug, Deserialize, ToSchema)]
pub struct TokenRequest {
    /// Always `password`.
    pub grant_type: String,
    /// Account username.
    pub username: String,
    /// Account password.
    pub password: String,
    /// OAuth2 client id.
    pub client_id: String,
    /// OAuth2 client secret.
    pub client_secret: String,
}

/// OAuth2 token response.
#[derive(Debug, Serialize, ToSchema)]
pub struct TokenResponse {
    /// Bearer token — pass as `Authorization: Bearer <access_token>`.
    pub access_token: String,
    /// Always `bearer`.
    pub token_type: String,
    /// Seconds until `access_token` expires.
    pub expires_in: i64,
}

/// One party on an expedition — sender (`clienteExpedidor`) or recipient
/// (`clienteDestinatario`), spec §11.2's Spanish field names.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ClienteInput {
    /// Full name.
    pub nombre: String,
    /// Street address.
    pub direccion: String,
    #[serde(rename = "codigoPostal")]
    /// Postal code.
    pub codigo_postal: String,
    /// City name.
    pub poblacion: String,
    /// ISO 3166-1 alpha-2 country code.
    pub pais: String,
    /// Contact phone number.
    pub telefono: Option<String>,
    /// Contact email.
    pub email: Option<String>,
}

/// One parcel (`bulto`).
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct BultoInput {
    /// Merchant-supplied parcel reference.
    pub referencia: Option<String>,
    /// Weight in kilograms (decimal, not grams — spec §11.2's own trait).
    pub peso: f64,
    /// Height, in centimeters.
    pub alto: i64,
    /// Width, in centimeters.
    pub ancho: i64,
    /// Length, in centimeters.
    pub largo: i64,
}

/// Request body for `POST /services/rest/tarifas`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TarifasRequest {
    /// Sender address.
    pub origen: ClienteInput,
    /// Recipient address.
    pub destino: ClienteInput,
    /// Parcels to ship together.
    pub bultos: Vec<BultoInput>,
    /// Declared value, in euro cents — used for the shared pricing
    /// engine's insurance surcharge, not part of the real dialect's own
    /// rating fields.
    #[serde(rename = "valorDeclarado")]
    pub valor_declarado: Option<i64>,
}

/// One priced service level.
#[derive(Debug, Serialize, ToSchema)]
pub struct TarifaOption {
    /// `1` (SEUR 24) or `2` (SEUR 10).
    pub servicio: String,
    /// Human-readable service name.
    pub nombre: String,
    /// Decimal euros (spec §11.2's own trait — not minor units).
    pub importe: f64,
    /// Always `EUR`.
    pub moneda: String,
    #[serde(rename = "diasEntrega")]
    /// Transit days.
    pub dias_entrega: i32,
}

/// Response body for `POST /services/rest/tarifas`.
#[derive(Debug, Serialize, ToSchema)]
pub struct TarifasResponse {
    /// Always `OK`.
    pub resultado: String,
    /// One priced option per service level.
    pub tarifas: Vec<TarifaOption>,
}

/// Request body for `POST /services/rest/expediciones` (spec §11.2's own
/// worked example).
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateExpedicionRequest {
    /// Cliente-centro-código, must match the authenticated credential.
    pub ccc: String,
    /// Sender's tax id.
    pub nif: Option<String>,
    /// `1` (SEUR 24) or `2` (SEUR 10).
    pub servicio: String,
    /// Product code — accepted but not used to vary behavior in this
    /// sandbox (`servicio` alone selects the tariff).
    pub producto: Option<String>,
    #[serde(rename = "totalBultos")]
    /// Number of parcels — must match `bultos.len()`.
    pub total_bultos: i32,
    #[serde(rename = "totalKilos")]
    /// Total declared weight, in kilograms.
    pub total_kilos: f64,
    /// Free-text delivery instructions.
    pub observaciones: Option<String>,
    #[serde(rename = "referenciaExpedicion")]
    /// Merchant order reference — also checked against spec §9.2's
    /// `SLOWSHIP`/`FASTSHIP` magic values.
    pub referencia_expedicion: String,
    #[serde(rename = "clienteExpedidor")]
    pub cliente_expedidor: ClienteInput,
    #[serde(rename = "clienteDestinatario")]
    pub cliente_destinatario: ClienteInput,
    /// Parcels to ship together.
    pub bultos: Vec<BultoInput>,
    /// `ZPL` (default) or `PDF`.
    #[serde(rename = "tipoEtiqueta")]
    pub tipo_etiqueta: Option<String>,
}

/// Per-expedition summary fields, nested inside `CreateExpedicionResponse`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ExpedicionSummary {
    #[serde(rename = "codigoExpedicion")]
    /// Same as the parent response's `ecb`.
    pub codigo_expedicion: String,
    /// `dd/MM/yyyy HH:mm`, in simulated time.
    #[serde(rename = "fechaAlta")]
    pub fecha_alta: String,
    /// Decimal euros.
    #[serde(rename = "importePortes")]
    pub importe_portes: f64,
    /// Always `EUR`.
    pub moneda: String,
}

/// One generated parcel label.
#[derive(Debug, Serialize, ToSchema)]
pub struct BultoOutput {
    #[serde(rename = "codigoBulto")]
    /// Per-parcel label code, derived from the expedition's `ecb`.
    pub codigo_bulto: String,
    /// Raw ZPL, or base64-encoded PDF — matching `tipoEtiqueta`.
    pub etiqueta: String,
}

/// Response body for `POST /services/rest/expediciones`.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateExpedicionResponse {
    /// `OK` or `KO` — carries the real outcome under an always-`200` HTTP
    /// status (spec §11.2's own trait).
    pub resultado: String,
    /// Present only when `resultado` is `KO`.
    pub descripcion: Option<String>,
    /// Expedition control code — Iberex's own tracking number. Empty on
    /// `KO`.
    pub ecb: String,
    /// Present only when `resultado` is `OK`.
    pub expedicion: Option<ExpedicionSummary>,
    /// One label per parcel. Empty on `KO`.
    pub bultos: Vec<BultoOutput>,
}

/// One tracking scan.
#[derive(Debug, Serialize, ToSchema)]
pub struct SeguimientoEvento {
    /// Spanish status code (spec §11.6's Iberex column).
    pub codigo: String,
    /// Human-readable description of this event.
    pub descripcion: String,
    /// `dd/MM/yyyy HH:mm`, in simulated time.
    pub fecha: String,
}

/// Response body for `GET /services/rest/seguimiento/{ecb}`.
#[derive(Debug, Serialize, ToSchema)]
pub struct SeguimientoResponse {
    /// Always `OK`.
    pub resultado: String,
    /// Expedition control code.
    pub ecb: String,
    /// Current Spanish status code (spec §11.6's Iberex column).
    pub estado: String,
    /// Every recorded scan, oldest first.
    pub eventos: Vec<SeguimientoEvento>,
}

/// Response body for `GET /services/rest/expediciones/{ecb}`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ExpedicionDetail {
    /// Always `OK`.
    pub resultado: String,
    /// Expedition control code.
    pub ecb: String,
    /// Current Spanish status code (spec §11.6's Iberex column).
    pub estado: String,
    pub expedicion: ExpedicionSummary,
}

/// Response body for `DELETE /services/rest/expediciones/{ecb}`.
#[derive(Debug, Serialize, ToSchema)]
pub struct CancelacionResponse {
    /// `OK` or `KO`.
    pub resultado: String,
    /// Present only when `resultado` is `KO`.
    pub descripcion: Option<String>,
}
