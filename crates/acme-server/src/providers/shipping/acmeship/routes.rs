//! Acme Ship handlers (spec §11.1). Dialect in, dialect out: every DB
//! access goes through `db::repo`, never `sqlx::query*` directly (spec §5).

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::Duration;

use crate::db::repo::shipments::{self, ShipmentRow};
use crate::domain::ids::{PickupId, RateOptionId, ShipmentId};
use crate::domain::scenario::{self, ShipmentInputs, ShipmentPackageDims, ShipmentScenario};
use crate::domain::shipment::{self, ShipmentCommand, ShipmentStatus};
use crate::error::{AcmeError, AcmeErrorBody, FieldError};
use crate::http::auth::AuthenticatedMerchant;
use crate::providers::shipping::acmeship::dto::*;
use crate::providers::shipping::acmeship::map;
use crate::sim::pricing::{self, Package, PricingInput, PricingResult, ServiceCode, Surcharge};
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

fn to_scenario_packages(packages: &[PackageInput]) -> Vec<ShipmentPackageDims> {
    packages
        .iter()
        .map(|p| ShipmentPackageDims {
            weight_grams: p.weight_grams,
            length_cm: p.length_cm,
            width_cm: p.width_cm,
            height_cm: p.height_cm,
        })
        .collect()
}

/// Folds spec §9.2's two pricing-relevant scenarios into an otherwise
/// scenario-agnostic `sim::pricing` result: `remote_area` adds a flat
/// surcharge and two transit days; `requires_insurance` is instead applied
/// by forcing `insurance_requested` before calling `pricing::quote` (see
/// call sites). `sim::pricing` itself stays scenario-agnostic — spec's own
/// worked example is tested independent of any scenario.
fn apply_remote_area(scenario: ShipmentScenario, mut result: PricingResult) -> PricingResult {
    if scenario == ShipmentScenario::RemoteArea {
        result.surcharges.push(Surcharge {
            code: "remote_area",
            label: "Remote area surcharge",
            cents: 350,
        });
        result.total_cents += 350;
        result.eta_max_days += 2;
    }
    result
}

fn estimate_co2_grams(billable_weight_grams: i64) -> i64 {
    (billable_weight_grams as f64 * 0.23).round() as i64
}

fn surcharges_json(surcharges: &[Surcharge]) -> String {
    serde_json::to_string(
        &surcharges
            .iter()
            .map(|s| (s.code, s.label, s.cents))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default()
}

fn to_rate_option_dto(
    id: String,
    service: ServiceCode,
    currency: &str,
    result: &PricingResult,
    now: chrono::DateTime<chrono::Utc>,
) -> RateOption {
    RateOption {
        id,
        carrier: "acmeship".to_string(),
        service_code: service.as_str().to_string(),
        service_name: service.display_name().to_string(),
        amount: MoneyOutput {
            value: result.total_cents,
            currency: currency.to_string(),
        },
        breakdown: PriceBreakdown {
            base: result.base_cents,
            surcharges: result
                .surcharges
                .iter()
                .map(|s| SurchargeOutput {
                    code: s.code.to_string(),
                    label: s.label.to_string(),
                    amount: s.cents,
                })
                .collect(),
        },
        billable_weight_grams: result.billable_weight_grams,
        estimated_delivery: EstimatedDelivery {
            min_days: result.eta_min_days as i32,
            max_days: result.eta_max_days as i32,
            eta: (now + Duration::days(result.eta_max_days as i64)).to_rfc3339(),
        },
        co2_grams: estimate_co2_grams(result.billable_weight_grams),
    }
}

#[utoipa::path(
    post, path = "/rates", operation_id = "create_rate", tag = "rates",
    description = "Rate a shipment: returns priced options for each service level.",
    request_body(content = RateRequest, description = "Origin, destination, packages and options to rate"),
    responses(
        (status = 200, description = "Rate options (empty when the destination has no coverage)", body = RateQuoteResponse),
        (status = 400, description = "Request failed validation", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn create_rate(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    headers: HeaderMap,
    Json(body): Json<RateRequest>,
) -> Result<Json<RateQuoteResponse>, AcmeError> {
    let explicit = explicit_scenario(&headers);
    let scenario_packages = to_scenario_packages(&body.packages);

    let resolved = scenario::resolve_shipment_scenario(&ShipmentInputs {
        destination_postal_code: &body.destination.postal_code,
        destination_country: &body.destination.country,
        packages: &scenario_packages,
        declared_value_cents: body.declared_value.amount,
        recipient_name: None,
        order_reference: None,
        explicit: explicit.as_deref(),
    })
    .map_err(|e| validation("metadata.acme_scenario", e.to_string()))?;

    let now = state.clock.now();
    let expires_at = now + Duration::minutes(30);
    let quote_id = shipments::create_rate_quote(
        &state.db,
        &shipments::NewRateQuote {
            merchant_id,
            provider_slug: "acmeship".to_string(),
            origin: map::address_json(&body.origin),
            destination: map::address_json(&body.destination),
            packages: serde_json::to_string(
                &body
                    .packages
                    .iter()
                    .map(|p| (p.weight_grams, p.length_cm, p.width_cm, p.height_cm))
                    .collect::<Vec<_>>(),
            )?,
            created_at: now,
            expires_at,
        },
    )
    .await?;

    if resolved == ShipmentScenario::NoCoverage {
        return Ok(Json(RateQuoteResponse {
            quote_id: quote_id.to_string(),
            expires_at: expires_at.to_rfc3339(),
            options: vec![],
        }));
    }

    let options_input = body.options.clone().unwrap_or_default();
    let pricing_packages: Vec<Package> =
        body.packages.iter().map(map::to_pricing_package).collect();
    let mut options = Vec::new();

    for service in ServiceCode::ALL {
        let result = pricing::quote(
            &service.tariff(),
            &PricingInput {
                origin_country: &body.origin.country,
                origin_postal: &body.origin.postal_code,
                destination_postal: &body.destination.postal_code,
                packages: &pricing_packages,
                declared_value_cents: body.declared_value.amount,
                residential: body.destination.residential.unwrap_or(false),
                insurance_requested: options_input.insurance.unwrap_or(false)
                    || resolved == ShipmentScenario::RequiresInsurance,
                cash_on_delivery: options_input.cash_on_delivery.unwrap_or(false),
                saturday_delivery: options_input.saturday_delivery.unwrap_or(false),
            },
        );
        let result = apply_remote_area(resolved, result);

        let option_id = shipments::create_rate_option(
            &state.db,
            &shipments::NewRateOption {
                quote_id,
                carrier_code: "acmeship".to_string(),
                service_code: service.as_str().to_string(),
                service_name: service.display_name().to_string(),
                amount_cents: result.total_cents,
                base_cents: result.base_cents,
                surcharges: surcharges_json(&result.surcharges),
                currency: body.declared_value.currency.clone(),
                eta_min_days: result.eta_min_days as i32,
                eta_max_days: result.eta_max_days as i32,
                billable_weight_grams: result.billable_weight_grams,
                insurance_cents: 0,
                co2_grams: Some(estimate_co2_grams(result.billable_weight_grams)),
            },
        )
        .await?;

        options.push(to_rate_option_dto(
            option_id.to_string(),
            service,
            &body.declared_value.currency,
            &result,
            now,
        ));
    }

    Ok(Json(RateQuoteResponse {
        quote_id: quote_id.to_string(),
        expires_at: expires_at.to_rfc3339(),
        options,
    }))
}

#[utoipa::path(
    post, path = "/shipments", operation_id = "create_shipment", tag = "shipments",
    description = "Create a shipment, from a prior rate_option_id or from raw parameters.",
    request_body(content = CreateShipmentRequest, description = "Create from a prior rate_option_id, or from raw origin/destination/packages"),
    responses(
        (status = 201, description = "Shipment created", body = Shipment),
        (status = 400, description = "Request failed validation", body = AcmeErrorBody),
        (status = 404, description = "rate_option_id does not exist or has expired", body = AcmeErrorBody),
        (status = 422, description = "Destination has no coverage, or a package exceeds size/weight limits (spec §9.2)", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn create_shipment(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    headers: HeaderMap,
    Json(body): Json<CreateShipmentRequest>,
) -> Result<(StatusCode, Json<Shipment>), AcmeError> {
    let now = state.clock.now();

    struct Booking {
        origin: String,
        destination: String,
        carrier_code: String,
        service_code: String,
        price_cents: i64,
        currency: String,
        eta_days: i64,
        declared_value_cents: i64,
    }

    let booking = if let Some(rate_option_id) = &body.rate_option_id {
        let option_id: RateOptionId = rate_option_id
            .parse()
            .map_err(|_| validation("rate_option_id", "malformed"))?;
        let option = shipments::get_rate_option(&state.db, option_id)
            .await?
            .ok_or(AcmeError::NotFound("rate option"))?;
        if option.merchant_id != merchant_id {
            return Err(AcmeError::NotFound("rate option"));
        }
        if option.expires_at < now {
            return Err(AcmeError::Conflict("rate option has expired".to_string()));
        }
        Booking {
            origin: option.origin,
            destination: option.destination,
            carrier_code: option.carrier_code,
            service_code: option.service_code,
            price_cents: option.amount_cents,
            currency: option.currency,
            eta_days: 3,
            // Not stored on `rate_options` (spec §7.4's schema) — a shipment
            // booked straight from a quote id carries no declared value of
            // its own in this milestone.
            declared_value_cents: 0,
        }
    } else {
        let origin = body
            .origin
            .as_ref()
            .ok_or_else(|| validation("origin", "required without rate_option_id"))?;
        let destination = body
            .destination
            .as_ref()
            .ok_or_else(|| validation("destination", "required without rate_option_id"))?;
        let packages = body
            .packages
            .as_ref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| validation("packages", "required without rate_option_id"))?;
        let declared_value = body
            .declared_value
            .as_ref()
            .ok_or_else(|| validation("declared_value", "required without rate_option_id"))?;
        let service = body.service_code.as_deref().unwrap_or("standard");
        let service_code = match service {
            "express_24h" => ServiceCode::Express24h,
            _ => ServiceCode::Standard,
        };

        let explicit = explicit_scenario(&headers);
        let scenario_packages = to_scenario_packages(packages);
        let resolved = scenario::resolve_shipment_scenario(&ShipmentInputs {
            destination_postal_code: &destination.postal_code,
            destination_country: &destination.country,
            packages: &scenario_packages,
            declared_value_cents: declared_value.amount,
            recipient_name: body.recipient_name.as_deref(),
            order_reference: body.order_reference.as_deref(),
            explicit: explicit.as_deref(),
        })
        .map_err(|e| validation("metadata.acme_scenario", e.to_string()))?;

        if matches!(
            resolved,
            ShipmentScenario::NoCoverage | ShipmentScenario::Oversized
        ) {
            return Err(AcmeError::UnprocessableState {
                from: "quoted".to_string(),
                to: "created".to_string(),
            });
        }

        let pricing_packages: Vec<Package> = packages.iter().map(map::to_pricing_package).collect();
        let result = pricing::quote(
            &service_code.tariff(),
            &PricingInput {
                origin_country: &origin.country,
                origin_postal: &origin.postal_code,
                destination_postal: &destination.postal_code,
                packages: &pricing_packages,
                declared_value_cents: declared_value.amount,
                residential: destination.residential.unwrap_or(false),
                insurance_requested: resolved == ShipmentScenario::RequiresInsurance,
                cash_on_delivery: false,
                saturday_delivery: false,
            },
        );
        let result = apply_remote_area(resolved, result);

        Booking {
            origin: map::address_json(origin),
            destination: map::address_json(destination),
            carrier_code: "acmeship".to_string(),
            service_code: service_code.as_str().to_string(),
            price_cents: result.total_cents,
            currency: declared_value.currency.clone(),
            eta_days: result.eta_max_days as i64,
            declared_value_cents: declared_value.amount,
        }
    };

    let tracking_number = format!("ACM{}", &ulid::Ulid::new().to_string()[..14]);
    let id = shipments::create(
        &state.db,
        &shipments::NewShipment {
            merchant_id,
            provider_slug: "acmeship".to_string(),
            carrier_code: booking.carrier_code,
            service_code: booking.service_code,
            origin: booking.origin,
            destination: booking.destination,
            tracking_number,
            price_cents: booking.price_cents,
            currency: booking.currency,
            created_at: now,
            eta_at: now + Duration::days(booking.eta_days.max(1)),
        },
    )
    .await?;
    let _ = booking.declared_value_cents; // not yet surfaced on `Shipment` (spec's response example omits it)

    let row = shipments::get(&state.db, id)
        .await?
        .ok_or(AcmeError::NotFound("shipment"))?;
    Ok((StatusCode::CREATED, Json(map::shipment_to_dto(&row))))
}

async fn load_owned(
    state: &AppState,
    merchant_id: crate::domain::ids::MerchantId,
    id: &str,
) -> Result<ShipmentRow, AcmeError> {
    let id: ShipmentId = id.parse().map_err(|_| AcmeError::NotFound("shipment"))?;
    shipments::get(&state.db, id)
        .await?
        .filter(|r| r.merchant_id == merchant_id)
        .ok_or(AcmeError::NotFound("shipment"))
}

#[utoipa::path(
    get, path = "/shipments/{id}", operation_id = "get_shipment", tag = "shipments",
    description = "Retrieve a shipment by id.",
    params(("id" = String, Path, description = "Shipment id, e.g. `shp_01J…`")),
    responses(
        (status = 200, description = "Shipment retrieved", body = Shipment),
        (status = 404, description = "No shipment with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn get_shipment(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<Shipment>, AcmeError> {
    let row = load_owned(&state, merchant_id, &id).await?;
    Ok(Json(map::shipment_to_dto(&row)))
}

#[utoipa::path(
    get, path = "/shipments", operation_id = "list_shipments", tag = "shipments",
    description = "List shipments for the authenticated merchant, newest first.",
    params(ListShipmentsQuery),
    responses((status = 200, description = "Page of shipments, newest first", body = ShipmentList)),
    security(("bearer_token" = []))
)]
pub async fn list_shipments(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Query(query): Query<ListShipmentsQuery>,
) -> Result<Json<ShipmentList>, AcmeError> {
    let limit = query.limit.unwrap_or(10).clamp(1, 100);
    let starting_after = query.starting_after.as_deref().and_then(|s| s.parse().ok());
    let status: Option<ShipmentStatus> = query
        .status
        .as_deref()
        .map(|s| serde_json::from_value(serde_json::Value::String(s.to_string())))
        .transpose()
        .map_err(|_| validation("status", "unknown shipment status"))?;

    let rows = shipments::list(
        &state.db,
        &shipments::ListFilter {
            merchant_id: Some(merchant_id),
            provider_slug: Some("acmeship".to_string()),
            status,
            search: None,
            limit: limit + 1,
            starting_after,
        },
    )
    .await?;

    let has_more = rows.len() as i64 > limit;
    let mut data: Vec<Shipment> = rows
        .iter()
        .take(limit as usize)
        .map(map::shipment_to_dto)
        .collect();
    let next_cursor = has_more
        .then(|| data.last().map(|s| s.id.clone()))
        .flatten();
    data.truncate(limit as usize);

    Ok(Json(ShipmentList {
        object: "list".to_string(),
        has_more,
        data,
        next_cursor,
    }))
}

#[utoipa::path(
    get, path = "/shipments/{id}/label", operation_id = "get_shipment_label", tag = "shipments",
    description = "Get (generating on first request) the shipping label for a shipment.",
    params(
        ("id" = String, Path, description = "Shipment id"),
        LabelQuery,
    ),
    responses(
        (status = 200, description = "Label location", body = LabelResponse),
        (status = 404, description = "No shipment with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn get_shipment_label(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
    Query(query): Query<LabelQuery>,
) -> Result<Json<LabelResponse>, AcmeError> {
    let row = load_owned(&state, merchant_id, &id).await?;
    let format = query.format.unwrap_or_else(|| "pdf".to_string());

    let url = match &row.label_url {
        Some(url) if row.label_format.as_deref() == Some(format.as_str()) => url.clone(),
        _ => {
            let url = format!("{}/labels/{}.{}", state.cfg.public_url, row.id, format);
            shipments::set_label(&state.db, row.id, &url, &format, state.clock.now()).await?;
            url
        }
    };

    Ok(Json(LabelResponse {
        shipment_id: row.id.to_string(),
        format,
        url,
    }))
}

#[utoipa::path(
    get, path = "/shipments/{id}/tracking", operation_id = "get_shipment_tracking", tag = "shipments",
    description = "Get the normalized tracking history for a shipment.",
    params(("id" = String, Path, description = "Shipment id")),
    responses(
        (status = 200, description = "Normalized tracking history", body = TrackingResponse),
        (status = 404, description = "No shipment with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn get_shipment_tracking(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<TrackingResponse>, AcmeError> {
    let row = load_owned(&state, merchant_id, &id).await?;
    let events = shipments::list_events(&state.db, row.id).await?;
    Ok(Json(TrackingResponse {
        tracking_number: row.tracking_number,
        status: row.status,
        events: events
            .into_iter()
            .map(|e| TrackingEvent {
                status: e.status,
                description: e.description,
                occurred_at: e.occurred_at.to_rfc3339(),
            })
            .collect(),
    }))
}

#[utoipa::path(
    get, path = "/tracking/{tracking_number}", operation_id = "public_tracking", tag = "tracking",
    description = "Look up tracking by carrier tracking number. No authentication required.",
    params(("tracking_number" = String, Path, description = "Carrier tracking number")),
    responses(
        (status = 200, description = "Normalized tracking history", body = TrackingResponse),
        (status = 404, description = "No shipment with that tracking number", body = AcmeErrorBody),
    )
)]
pub async fn public_tracking(
    State(state): State<AppState>,
    Path(tracking_number): Path<String>,
) -> Result<Json<TrackingResponse>, AcmeError> {
    let row = shipments::get_by_tracking(&state.db, "acmeship", &tracking_number)
        .await?
        .ok_or(AcmeError::NotFound("shipment"))?;
    let events = shipments::list_events(&state.db, row.id).await?;
    Ok(Json(TrackingResponse {
        tracking_number: row.tracking_number,
        status: row.status,
        events: events
            .into_iter()
            .map(|e| TrackingEvent {
                status: e.status,
                description: e.description,
                occurred_at: e.occurred_at.to_rfc3339(),
            })
            .collect(),
    }))
}

#[utoipa::path(
    post, path = "/shipments/{id}/cancel", operation_id = "cancel_shipment", tag = "shipments",
    description = "Cancel a shipment before it has been picked up.",
    params(("id" = String, Path, description = "Shipment id")),
    responses(
        (status = 200, description = "Shipment cancelled", body = Shipment),
        (status = 404, description = "No shipment with that id", body = AcmeErrorBody),
        (status = 409, description = "Shipment has already been picked up or later", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn cancel_shipment(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<Shipment>, AcmeError> {
    let row = load_owned(&state, merchant_id, &id).await?;
    if !matches!(
        row.status,
        ShipmentStatus::Created | ShipmentStatus::LabelGenerated | ShipmentStatus::PickedUp
    ) {
        return Err(AcmeError::UnprocessableState {
            from: format!("{:?}", row.status).to_lowercase(),
            to: "cancelled".to_string(),
        });
    }

    let transition = shipment::apply(row.status, ShipmentCommand::Cancel, &state.clock)?;
    let seq = shipments::event_count(&state.db, row.id).await?;
    shipments::advance(
        &state.db,
        row.id,
        merchant_id,
        "acmeship",
        transition.to,
        transition.event,
        seq,
        transition.occurred_at,
        None,
        None,
    )
    .await?;

    let row = shipments::get(&state.db, row.id)
        .await?
        .ok_or(AcmeError::NotFound("shipment"))?;
    Ok(Json(map::shipment_to_dto(&row)))
}

#[utoipa::path(
    post, path = "/pickups", operation_id = "create_pickup", tag = "pickups",
    description = "Schedule a collection request for one or more shipments.",
    request_body(content = CreatePickupRequest, description = "Collection request"),
    responses(
        (status = 201, description = "Pickup scheduled", body = Pickup),
        (status = 400, description = "Request failed validation", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn create_pickup(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Json(body): Json<CreatePickupRequest>,
) -> Result<(StatusCode, Json<Pickup>), AcmeError> {
    let window_start = chrono::DateTime::parse_from_rfc3339(&body.window_start)
        .map_err(|_| validation("window_start", "must be RFC3339"))?
        .with_timezone(&chrono::Utc);
    let window_end = chrono::DateTime::parse_from_rfc3339(&body.window_end)
        .map_err(|_| validation("window_end", "must be RFC3339"))?
        .with_timezone(&chrono::Utc);
    let now = state.clock.now();

    let id = shipments::create_pickup(
        &state.db,
        &shipments::NewPickup {
            merchant_id,
            provider_slug: "acmeship".to_string(),
            address: map::address_json(&body.address),
            window_start,
            window_end,
            shipment_ids: serde_json::to_string(&body.shipment_ids)?,
            created_at: now,
        },
    )
    .await?;

    let row = shipments::get_pickup(&state.db, merchant_id, id)
        .await?
        .ok_or(AcmeError::NotFound("pickup"))?;
    Ok((
        StatusCode::CREATED,
        Json(Pickup {
            id: row.id.to_string(),
            status: row.status,
            confirmation_code: row.confirmation_code,
            window_start: row.window_start.to_rfc3339(),
            window_end: row.window_end.to_rfc3339(),
            created_at: row.created_at.to_rfc3339(),
        }),
    ))
}

#[utoipa::path(
    get, path = "/pickups/{id}", operation_id = "get_pickup", tag = "pickups",
    description = "Retrieve a pickup by id.",
    params(("id" = String, Path, description = "Pickup id")),
    responses(
        (status = 200, description = "Pickup retrieved", body = Pickup),
        (status = 404, description = "No pickup with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn get_pickup(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<Pickup>, AcmeError> {
    let id: PickupId = id.parse().map_err(|_| AcmeError::NotFound("pickup"))?;
    let row = shipments::get_pickup(&state.db, merchant_id, id)
        .await?
        .ok_or(AcmeError::NotFound("pickup"))?;
    Ok(Json(Pickup {
        id: row.id.to_string(),
        status: row.status,
        confirmation_code: row.confirmation_code,
        window_start: row.window_start.to_rfc3339(),
        window_end: row.window_end.to_rfc3339(),
        created_at: row.created_at.to_rfc3339(),
    }))
}

#[utoipa::path(
    post, path = "/returns", operation_id = "create_return", tag = "returns",
    description = "Generate a return label for an existing shipment.",
    request_body(content = CreateReturnRequest, description = "Shipment to generate a return label for"),
    responses(
        (status = 201, description = "Return label created", body = ReturnResponse),
        (status = 404, description = "No shipment with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn create_return(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Json(body): Json<CreateReturnRequest>,
) -> Result<(StatusCode, Json<ReturnResponse>), AcmeError> {
    let row = load_owned(&state, merchant_id, &body.shipment_id).await?;
    let return_tracking_number = format!("RET{}", &ulid::Ulid::new().to_string()[..14]);
    let label_url = format!("{}/labels/{}-return.pdf", state.cfg.public_url, row.id);

    shipments::set_return_tracking(
        &state.db,
        row.id,
        &return_tracking_number,
        state.clock.now(),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(ReturnResponse {
            shipment_id: row.id.to_string(),
            return_tracking_number,
            label_url,
        }),
    ))
}

#[utoipa::path(
    get, path = "/coverage", operation_id = "get_coverage", tag = "coverage",
    description = "Check serviceability and available services for one destination.",
    params(CoverageQuery),
    responses((status = 200, description = "Serviceability for one destination", body = CoverageResponse)),
    security(("bearer_token" = []))
)]
pub async fn get_coverage(
    Extension(AuthenticatedMerchant(_merchant_id)): Extension<AuthenticatedMerchant>,
    Query(query): Query<CoverageQuery>,
) -> Result<Json<CoverageResponse>, AcmeError> {
    let resolved = scenario::resolve_shipment_scenario(&ShipmentInputs {
        destination_postal_code: &query.postal_code,
        destination_country: &query.country,
        packages: &[],
        declared_value_cents: 0,
        recipient_name: None,
        order_reference: None,
        explicit: None,
    })
    .map_err(|e| validation("postal_code", e.to_string()))?;

    let response = match resolved {
        ShipmentScenario::NoCoverage => CoverageResponse {
            supported: false,
            services: vec![],
            extra_days: 0,
            surcharge_cents: 0,
        },
        ShipmentScenario::RemoteArea => CoverageResponse {
            supported: true,
            services: ServiceCode::ALL
                .iter()
                .map(|s| s.as_str().to_string())
                .collect(),
            extra_days: 2,
            surcharge_cents: 350,
        },
        _ => CoverageResponse {
            supported: true,
            services: ServiceCode::ALL
                .iter()
                .map(|s| s.as_str().to_string())
                .collect(),
            extra_days: 0,
            surcharge_cents: 0,
        },
    };

    Ok(Json(response))
}

#[utoipa::path(
    get, path = "/services", operation_id = "list_services", tag = "coverage",
    description = "List the service levels this provider offers.",
    responses((status = 200, description = "Service catalog", body = ServicesResponse)),
    security(("bearer_token" = []))
)]
pub async fn list_services(
    Extension(AuthenticatedMerchant(_merchant_id)): Extension<AuthenticatedMerchant>,
) -> Json<ServicesResponse> {
    Json(ServicesResponse {
        data: ServiceCode::ALL
            .iter()
            .map(|s| {
                let (min, max) = s.eta_days();
                ServiceCatalogEntry {
                    code: s.as_str().to_string(),
                    name: s.display_name().to_string(),
                    eta_min_days: min as i32,
                    eta_max_days: max as i32,
                }
            })
            .collect(),
    })
}
