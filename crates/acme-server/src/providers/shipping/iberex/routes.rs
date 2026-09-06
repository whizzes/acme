//! Iberex Express handlers (spec §11.2). Dialect in, dialect out: every DB
//! access goes through `db::repo`, never `sqlx::query*` directly (spec §5).
//!
//! Trimmed from spec §11.2's full route table (specs/006-Dialects.md item
//! 3): pickups (`POST /services/rest/recogidas`) and coverage
//! (`GET /services/rest/cobertura`) are not implemented this slice —
//! rating, label creation, tracking, and cancel are.

use axum::Json;
use axum::extract::{Extension, Form, Path, State};
use axum::http::HeaderMap;

use crate::db::repo::shipments;
use crate::domain::scenario::{self, ShipmentInputs, ShipmentPackageDims, ShipmentScenario};
use crate::domain::shipment::{self, ShipmentCommand, ShipmentStatus};
use crate::error::{AcmeError, AcmeErrorBody, FieldError};
use crate::http::auth::AuthenticatedMerchant;
use crate::providers::shipping::iberex::dto::*;
use crate::providers::shipping::iberex::map::{self, IberexService};
use crate::sim::pricing::{self, Package, PricingInput};
use crate::state::AppState;

fn validation(param: &str, message: impl Into<String>) -> AcmeError {
    AcmeError::Validation(vec![FieldError {
        param: param.to_string(),
        message: message.into(),
    }])
}

fn explicit_scenario(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-acme-scenario")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn to_scenario_packages(bultos: &[BultoInput]) -> Vec<ShipmentPackageDims> {
    bultos
        .iter()
        .map(|b| ShipmentPackageDims {
            weight_grams: (b.peso * 1000.0).round() as i64,
            length_cm: b.largo,
            width_cm: b.ancho,
            height_cm: b.alto,
        })
        .collect()
}

#[utoipa::path(
    post, path = "/services/oauth2/token", operation_id = "iberex_oauth_token", tag = "auth",
    description = "OAuth2 password grant — exchanges username/password/client credentials for a short-lived bearer token.",
    request_body(content = TokenRequest, description = "Password grant parameters"),
    responses(
        (status = 200, description = "Access token issued", body = TokenResponse),
        (status = 401, description = "Invalid credentials", body = AcmeErrorBody),
    )
)]
pub async fn oauth_token(
    State(state): State<AppState>,
    Form(body): Form<TokenRequest>,
) -> Result<Json<TokenResponse>, AcmeError> {
    if body.grant_type != "password" {
        return Err(validation("grant_type", "must be \"password\""));
    }

    let merchant_id = crate::http::auth::validate_oauth2_password_grant(
        &state.db,
        "iberex",
        &body.client_id,
        &body.client_secret,
        &body.username,
        &body.password,
    )
    .await?
    .ok_or(AcmeError::Unauthorized("invalid credentials"))?;

    let now = state.clock.now();
    let ttl = chrono::Duration::hours(1);
    let (access_token, _expires_at) =
        crate::db::repo::oauth_tokens::issue(&state.db, merchant_id, "iberex", ttl, now).await?;

    Ok(Json(TokenResponse {
        access_token,
        token_type: "bearer".to_string(),
        expires_in: ttl.num_seconds(),
    }))
}

#[utoipa::path(
    post, path = "/services/rest/tarifas", operation_id = "iberex_tarifas", tag = "tarifas",
    description = "Rate a shipment across both SEUR service levels.",
    request_body(content = TarifasRequest, description = "Origin, destination and parcels to rate"),
    responses((status = 200, description = "Priced service levels", body = TarifasResponse)),
    security(("iberex_oauth2" = []))
)]
pub async fn create_tarifa(
    Extension(AuthenticatedMerchant(_merchant_id)): Extension<AuthenticatedMerchant>,
    Json(body): Json<TarifasRequest>,
) -> Result<Json<TarifasResponse>, AcmeError> {
    let pricing_packages: Vec<Package> = body.bultos.iter().map(map::to_pricing_package).collect();
    let mut tarifas = Vec::new();

    for service in IberexService::ALL {
        let result = pricing::quote(
            &service.tariff(),
            &PricingInput {
                origin_country: &body.origen.pais,
                origin_postal: &body.origen.codigo_postal,
                destination_postal: &body.destino.codigo_postal,
                packages: &pricing_packages,
                declared_value_cents: body.valor_declarado.unwrap_or(0),
                residential: false,
                insurance_requested: body.valor_declarado.is_some(),
                cash_on_delivery: false,
                saturday_delivery: false,
            },
        );
        tarifas.push(TarifaOption {
            servicio: service.servicio_code().to_string(),
            nombre: service.display_name().to_string(),
            importe: result.total_cents as f64 / 100.0,
            moneda: "EUR".to_string(),
            dias_entrega: result.eta_max_days as i32,
        });
    }

    Ok(Json(TarifasResponse {
        resultado: "OK".to_string(),
        tarifas,
    }))
}

fn ko_response(descripcion: &str) -> CreateExpedicionResponse {
    CreateExpedicionResponse {
        resultado: "KO".to_string(),
        descripcion: Some(descripcion.to_string()),
        ecb: String::new(),
        expedicion: None,
        bultos: vec![],
    }
}

#[utoipa::path(
    post, path = "/services/rest/expediciones", operation_id = "create_expedicion", tag = "expediciones",
    description = "Create an expedition: books the shipment and generates its labels in one call (spec §11.2).",
    request_body(content = CreateExpedicionRequest, description = "Expedition to create"),
    responses((status = 200, description = "resultado OK with the expedition and labels, or resultado KO with a description", body = CreateExpedicionResponse)),
    security(("iberex_oauth2" = []))
)]
pub async fn create_expedicion(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    headers: HeaderMap,
    Json(body): Json<CreateExpedicionRequest>,
) -> Result<Json<CreateExpedicionResponse>, AcmeError> {
    let Some(service) = IberexService::from_servicio(&body.servicio) else {
        return Ok(Json(ko_response("servicio desconocido")));
    };

    let explicit = explicit_scenario(&headers);
    let scenario_packages = to_scenario_packages(&body.bultos);
    let resolved = scenario::resolve_shipment_scenario(&ShipmentInputs {
        destination_postal_code: &body.cliente_destinatario.codigo_postal,
        destination_country: &body.cliente_destinatario.pais,
        packages: &scenario_packages,
        declared_value_cents: 0,
        recipient_name: Some(&body.cliente_destinatario.nombre),
        order_reference: Some(&body.referencia_expedicion),
        explicit: explicit.as_deref(),
    })
    .map_err(|e| validation("metadata.acme_scenario", e.to_string()))?;

    if matches!(
        resolved,
        ShipmentScenario::NoCoverage | ShipmentScenario::Oversized
    ) {
        return Ok(Json(ko_response(
            "sin cobertura para el destino, o bulto fuera de límites",
        )));
    }

    let now = state.clock.now();
    let pricing_packages: Vec<Package> = body.bultos.iter().map(map::to_pricing_package).collect();
    let result = pricing::quote(
        &service.tariff(),
        &PricingInput {
            origin_country: &body.cliente_expedidor.pais,
            origin_postal: &body.cliente_expedidor.codigo_postal,
            destination_postal: &body.cliente_destinatario.codigo_postal,
            packages: &pricing_packages,
            declared_value_cents: 0,
            residential: false,
            insurance_requested: resolved == ShipmentScenario::RequiresInsurance,
            cash_on_delivery: false,
            saturday_delivery: false,
        },
    );

    let ecb = format!("08{}", &ulid::Ulid::new().to_string()[..14]);
    let id = shipments::create(
        &state.db,
        &shipments::NewShipment {
            merchant_id,
            provider_slug: "iberex".to_string(),
            carrier_code: "iberex".to_string(),
            service_code: service.servicio_code().to_string(),
            origin: map::cliente_json(&body.cliente_expedidor),
            destination: map::cliente_json(&body.cliente_destinatario),
            tracking_number: ecb.clone(),
            price_cents: result.total_cents,
            currency: "EUR".to_string(),
            created_at: now,
            eta_at: now + chrono::Duration::days(result.eta_max_days.max(1) as i64),
        },
    )
    .await?;

    let tipo_etiqueta = body.tipo_etiqueta.as_deref().unwrap_or("ZPL");
    let bultos: Vec<BultoOutput> = body
        .bultos
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let codigo_bulto = format!("{ecb}{:03}", i + 1);
            let etiqueta = if tipo_etiqueta.eq_ignore_ascii_case("PDF") {
                map::pdf_label_base64(&codigo_bulto)
            } else {
                map::zpl_label(&codigo_bulto)
            };
            BultoOutput {
                codigo_bulto,
                etiqueta,
            }
        })
        .collect();
    let label_url = format!("{}/labels/{}.{}", state.cfg.public_url, id, tipo_etiqueta);
    shipments::set_label(&state.db, id, &label_url, tipo_etiqueta, now).await?;

    Ok(Json(CreateExpedicionResponse {
        resultado: "OK".to_string(),
        descripcion: None,
        ecb: ecb.clone(),
        expedicion: Some(ExpedicionSummary {
            codigo_expedicion: ecb,
            fecha_alta: map::fecha_dd_mm_yyyy_hh_mm(now),
            importe_portes: result.total_cents as f64 / 100.0,
            moneda: "EUR".to_string(),
        }),
        bultos,
    }))
}

async fn load_by_ecb(
    state: &AppState,
    ecb: &str,
) -> Result<crate::db::repo::shipments::ShipmentRow, AcmeError> {
    shipments::get_by_tracking(&state.db, "iberex", ecb)
        .await?
        .ok_or(AcmeError::NotFound("expedition"))
}

#[utoipa::path(
    get, path = "/services/rest/expediciones/{ecb}", operation_id = "get_expedicion", tag = "expediciones",
    description = "Retrieve an expedition by its ECB.",
    params(("ecb" = String, Path, description = "Expedition control code")),
    responses(
        (status = 200, description = "Expedition retrieved", body = ExpedicionDetail),
        (status = 404, description = "No expedition with that ECB", body = AcmeErrorBody),
    ),
    security(("iberex_oauth2" = []))
)]
pub async fn get_expedicion(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(_merchant_id)): Extension<AuthenticatedMerchant>,
    Path(ecb): Path<String>,
) -> Result<Json<ExpedicionDetail>, AcmeError> {
    let row = load_by_ecb(&state, &ecb).await?;
    Ok(Json(ExpedicionDetail {
        resultado: "OK".to_string(),
        ecb: row.tracking_number.clone(),
        estado: map::iberex_status(row.status).to_string(),
        expedicion: ExpedicionSummary {
            codigo_expedicion: row.tracking_number,
            fecha_alta: map::fecha_dd_mm_yyyy_hh_mm(row.created_at),
            importe_portes: row.price_cents as f64 / 100.0,
            moneda: row.currency,
        },
    }))
}

#[utoipa::path(
    delete, path = "/services/rest/expediciones/{ecb}", operation_id = "cancel_expedicion", tag = "expediciones",
    description = "Cancel an expedition before it has been picked up.",
    params(("ecb" = String, Path, description = "Expedition control code")),
    responses(
        (status = 200, description = "resultado OK if cancelled, resultado KO if not cancellable", body = CancelacionResponse),
        (status = 404, description = "No expedition with that ECB", body = AcmeErrorBody),
    ),
    security(("iberex_oauth2" = []))
)]
pub async fn cancel_expedicion(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(ecb): Path<String>,
) -> Result<Json<CancelacionResponse>, AcmeError> {
    let row = load_by_ecb(&state, &ecb).await?;
    if !matches!(
        row.status,
        ShipmentStatus::Created | ShipmentStatus::LabelGenerated | ShipmentStatus::PickedUp
    ) {
        return Ok(Json(CancelacionResponse {
            resultado: "KO".to_string(),
            descripcion: Some("la expedición ya ha sido recogida o finalizada".to_string()),
        }));
    }

    let transition = shipment::apply(row.status, ShipmentCommand::Cancel, &state.clock)?;
    let seq = shipments::event_count(&state.db, row.id).await?;
    shipments::advance(
        &state.db,
        row.id,
        merchant_id,
        "iberex",
        transition.to,
        transition.event,
        seq,
        transition.occurred_at,
        None,
        None,
    )
    .await?;

    Ok(Json(CancelacionResponse {
        resultado: "OK".to_string(),
        descripcion: None,
    }))
}

#[utoipa::path(
    get, path = "/services/rest/seguimiento/{ecb}", operation_id = "seguimiento_expedicion", tag = "expediciones",
    description = "Track an expedition by its ECB.",
    params(("ecb" = String, Path, description = "Expedition control code")),
    responses(
        (status = 200, description = "Tracking history", body = SeguimientoResponse),
        (status = 404, description = "No expedition with that ECB", body = AcmeErrorBody),
    ),
    security(("iberex_oauth2" = []))
)]
pub async fn seguimiento_expedicion(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(_merchant_id)): Extension<AuthenticatedMerchant>,
    Path(ecb): Path<String>,
) -> Result<Json<SeguimientoResponse>, AcmeError> {
    let row = load_by_ecb(&state, &ecb).await?;
    let events = shipments::list_events(&state.db, row.id).await?;

    Ok(Json(SeguimientoResponse {
        resultado: "OK".to_string(),
        ecb: row.tracking_number,
        estado: map::iberex_status(row.status).to_string(),
        eventos: events
            .into_iter()
            .map(|e| SeguimientoEvento {
                codigo: map::iberex_status(e.status).to_string(),
                descripcion: e.description,
                fecha: map::fecha_dd_mm_yyyy_hh_mm(e.occurred_at),
            })
            .collect(),
    }))
}
