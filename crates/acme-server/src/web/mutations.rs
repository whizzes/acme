//! Dashboard mutations (spec §13.3): advance/refund/exception. Each
//! returns the freshly re-rendered detail fragment, an HTMX outerHTML
//! swap of the whole detail body (spec §13.4 pattern 3, scaled up from
//! "swap one row" — a resource detail page has no row of its own).

use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::dashboard::dispatcher;
use crate::db::repo::faults;
use crate::db::repo::payments::{self, NewRefund};
use crate::db::repo::providers;
use crate::db::repo::shipments;
use crate::db::repo::sim as sim_settings;
use crate::db::repo::webhooks::{self, DueDelivery};
use crate::domain::ids::{FaultId, PaymentId, ShipmentId, WebhookDeliveryId};
use crate::domain::money::{Currency, Money};
use crate::domain::payment::{self, PaymentCommand, PaymentStatus};
use crate::domain::shipment::{self, ShipmentCommand};
use crate::error::{AcmeError, AppError};
use crate::providers::payments::acmepay::dto::{CardInput, PaymentMethodInput};
use crate::providers::payments::acmepay::routes as acmepay_routes;
use crate::providers::shipping::acmeship::dto::{AddressInput, MoneyInput, PackageInput};
use crate::providers::shipping::acmeship::routes as acmeship_routes;
use crate::state::AppState;
use crate::web::pages::components;
use crate::web::pages::{
    payments as payments_page, providers as providers_page, shipments as shipments_page,
    simulator as simulator_page, webhooks as webhooks_page,
};

/// Renders an `AcmeError` a "Try it" form's own inputs can cause
/// (validation, no-coverage/oversized) as the inline message the spec
/// requires; any other variant is a genuine bug, so it still becomes a 500
/// via `AppError` rather than being shown to the merchant as if it were
/// their mistake.
fn acme_error_message(err: AcmeError) -> Result<String, AppError> {
    match err {
        AcmeError::Validation(errors) => Ok(errors
            .first()
            .map(|e| format!("{}: {}", e.param, e.message))
            .unwrap_or_else(|| "validation failed".to_string())),
        AcmeError::UnprocessableState { .. } => {
            Ok("destination has no coverage, or a package exceeds size/weight limits".to_string())
        }
        AcmeError::Conflict(message) => Ok(message),
        AcmeError::ProviderDown => Ok("simulated provider error".to_string()),
        other => {
            Err(anyhow::anyhow!("unexpected error creating from a Try it form: {other:?}").into())
        }
    }
}

#[derive(Deserialize)]
pub struct AdvanceBody {
    pub to_status: String,
}

fn parse_status<T: serde::de::DeserializeOwned>(raw: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

pub async fn advance_payment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(body): Form<AdvanceBody>,
) -> Result<Response, AppError> {
    let Ok(id) = id.parse::<PaymentId>() else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(row) = payments::get(&state.db, id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(to) = parse_status::<PaymentStatus>(&body.to_status) else {
        return Ok((StatusCode::BAD_REQUEST, "unknown status").into_response());
    };
    let Some(cmd) = row.status.command_for_transition(to) else {
        return Ok((StatusCode::CONFLICT, "illegal transition").into_response());
    };

    let transition = payment::apply(row.status, cmd, &state.clock)?;
    let seq = payments::event_count(&state.db, id).await?;
    payments::advance(
        &state.db,
        id,
        row.merchant_id,
        &row.provider_slug,
        transition.to,
        transition.event,
        seq,
        transition.occurred_at,
        None,
        None,
    )
    .await?;

    let row = payments::get(&state.db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment vanished mid-mutation"))?;
    Ok(payments_page::detail_fragment(&state, &row)
        .await?
        .into_response())
}

#[derive(Deserialize, Default)]
pub struct RefundBody {
    pub amount_cents: Option<i64>,
}

/// Mirrors `providers::payments::acmepay::routes::create_refund`'s
/// choreography: `RefundPartially` first, then an immediate `Refund` hop
/// when the remaining balance reaches zero, then the `refunds` row.
pub async fn refund_payment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(body): Form<RefundBody>,
) -> Result<Response, AppError> {
    let Ok(id) = id.parse::<PaymentId>() else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(row) = payments::get(&state.db, id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    let remaining_before = row.amount_cents - row.amount_refunded_cents;
    let amount = body
        .amount_cents
        .filter(|a| *a > 0)
        .unwrap_or(remaining_before);
    if amount <= 0 || amount > remaining_before {
        return Ok((
            StatusCode::BAD_REQUEST,
            "amount exceeds the refundable balance",
        )
            .into_response());
    }

    let now = state.clock.now();
    let Ok(currency) = row.currency.parse::<Currency>() else {
        return Err(anyhow::anyhow!("unknown stored currency").into());
    };
    let remaining_after = remaining_before - amount;
    let seq = payments::event_count(&state.db, id).await?;

    match row.status {
        PaymentStatus::Captured => {
            let transition = payment::apply(
                PaymentStatus::Captured,
                PaymentCommand::RefundPartially {
                    amount: Money::new(amount, currency),
                },
                &state.clock,
            )?;
            payments::advance(
                &state.db,
                id,
                row.merchant_id,
                &row.provider_slug,
                transition.to,
                transition.event,
                seq,
                transition.occurred_at,
                None,
                None,
            )
            .await?;
            if remaining_after == 0 {
                let transition2 =
                    payment::apply(transition.to, PaymentCommand::Refund, &state.clock)?;
                payments::advance(
                    &state.db,
                    id,
                    row.merchant_id,
                    &row.provider_slug,
                    transition2.to,
                    transition2.event,
                    seq + 1,
                    transition2.occurred_at,
                    None,
                    None,
                )
                .await?;
            }
        }
        PaymentStatus::PartiallyRefunded if remaining_after == 0 => {
            let transition = payment::apply(
                PaymentStatus::PartiallyRefunded,
                PaymentCommand::Refund,
                &state.clock,
            )?;
            payments::advance(
                &state.db,
                id,
                row.merchant_id,
                &row.provider_slug,
                transition.to,
                transition.event,
                seq,
                transition.occurred_at,
                None,
                None,
            )
            .await?;
        }
        PaymentStatus::PartiallyRefunded => {
            // No further status transition — spec §8.1 has no
            // `partially_refunded -> partially_refunded` edge; only the
            // refund row and the running total change.
        }
        _ => {
            return Ok((
                StatusCode::CONFLICT,
                "payment is not refundable in its current status",
            )
                .into_response());
        }
    }

    payments::create_refund(
        &state.db,
        &NewRefund {
            payment_id: id,
            amount_cents: amount,
            reason: Some("dashboard refund".to_string()),
            created_at: now,
        },
        row.merchant_id,
        &row.provider_slug,
    )
    .await?;

    let row = payments::get(&state.db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment vanished mid-mutation"))?;
    Ok(payments_page::detail_fragment(&state, &row)
        .await?
        .into_response())
}

pub async fn advance_shipment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(body): Form<AdvanceBody>,
) -> Result<Response, AppError> {
    let Ok(id) = id.parse::<ShipmentId>() else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(row) = shipments::get(&state.db, id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(to) = parse_status(&body.to_status) else {
        return Ok((StatusCode::BAD_REQUEST, "unknown status").into_response());
    };
    let Some(cmd) = row.status.command_for_transition(to) else {
        return Ok((StatusCode::CONFLICT, "illegal transition").into_response());
    };

    let transition = shipment::apply(row.status, cmd, &state.clock)?;
    let seq = shipments::event_count(&state.db, id).await?;
    shipments::advance(
        &state.db,
        id,
        row.merchant_id,
        &row.provider_slug,
        transition.to,
        transition.event,
        seq,
        transition.occurred_at,
        None,
        None,
    )
    .await?;

    let row = shipments::get(&state.db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("shipment vanished mid-mutation"))?;
    Ok(shipments_page::detail_fragment(&state, &row)
        .await?
        .into_response())
}

#[derive(Deserialize)]
pub struct ExceptionBody {
    pub reason: String,
}

pub async fn except_shipment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(body): Form<ExceptionBody>,
) -> Result<Response, AppError> {
    let Ok(id) = id.parse::<ShipmentId>() else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(row) = shipments::get(&state.db, id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Ok(transition) = shipment::apply(row.status, ShipmentCommand::Except, &state.clock) else {
        return Ok((
            StatusCode::CONFLICT,
            "cannot declare an exception from this status",
        )
            .into_response());
    };

    let seq = shipments::event_count(&state.db, id).await?;
    let reason = Some(body.reason.trim()).filter(|r| !r.is_empty());
    shipments::advance(
        &state.db,
        id,
        row.merchant_id,
        &row.provider_slug,
        transition.to,
        transition.event,
        seq,
        transition.occurred_at,
        None,
        reason,
    )
    .await?;

    let row = shipments::get(&state.db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("shipment vanished mid-mutation"))?;
    Ok(shipments_page::detail_fragment(&state, &row)
        .await?
        .into_response())
}

/// Loads a delivery and adapts it into `DueDelivery`, ignoring its
/// `next_attempt_at` gating — a manual retry/resend is deliberately
/// out-of-schedule (spec §13.3).
async fn load_forceable_delivery(
    state: &AppState,
    id: WebhookDeliveryId,
) -> Result<Option<DueDelivery>, AppError> {
    let Some(detail) = webhooks::get_delivery(&state.db, id).await? else {
        return Ok(None);
    };
    Ok(DueDelivery::from_detail(detail))
}

pub async fn retry_delivery(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Ok(id) = id.parse::<WebhookDeliveryId>() else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(delivery) = load_forceable_delivery(&state, id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    tracing::info!(delivery_id = %id, endpoint_id = %delivery.endpoint_id, "dashboard: manual retry requested");
    let now = state.clock.now();
    dispatcher::attempt_delivery(&state, &delivery, now).await;

    let Some(fragment) = webhooks_page::delivery_fragment(&state, id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    Ok(fragment.into_response())
}

pub async fn resend_delivery(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Ok(id) = id.parse::<WebhookDeliveryId>() else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(delivery) = load_forceable_delivery(&state, id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    tracing::info!(delivery_id = %id, endpoint_id = %delivery.endpoint_id, "dashboard: manual resend (byte-identical replay) requested");
    let now = state.clock.now();
    dispatcher::resend_delivery(&state, &delivery, now).await;

    let Some(fragment) = webhooks_page::delivery_fragment(&state, id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    Ok(fragment.into_response())
}

#[derive(Deserialize)]
pub struct SimClockForm {
    /// `set_speed`, `pause`, `resume`, or `jump`.
    pub action: String,
    pub multiplier: Option<f64>,
    pub jump_seconds: Option<i64>,
}

/// `POST /sim/clock` (spec §13.3, specs/008-Simulation.md item 5): wires
/// the dashboard to `SimClock::set_multiplier`/`jump`, both fully built
/// and unit-tested since M1 with no caller until now. See
/// `db::repo::sim::save_clock`'s doc comment for why `pause` doesn't
/// simply persist the clock's now-zero effective rate.
pub async fn set_sim_clock(
    State(state): State<AppState>,
    Form(body): Form<SimClockForm>,
) -> Result<Response, AppError> {
    let settings = sim_settings::get(&state.db).await?;
    let now = state.clock.now();

    match body.action.as_str() {
        "set_speed" => {
            let multiplier = body.multiplier.unwrap_or(settings.multiplier);
            state.clock.set_multiplier(multiplier);
            sim_settings::save_clock(&state.db, state.clock.now(), multiplier, false, now).await?;
        }
        "pause" => {
            state.clock.set_multiplier(0.0);
            sim_settings::save_clock(&state.db, state.clock.now(), settings.multiplier, true, now)
                .await?;
        }
        "resume" => {
            state.clock.set_multiplier(settings.multiplier);
            sim_settings::save_clock(
                &state.db,
                state.clock.now(),
                settings.multiplier,
                false,
                now,
            )
            .await?;
        }
        "jump" => {
            let seconds = body.jump_seconds.unwrap_or(0);
            state.clock.jump(chrono::Duration::seconds(seconds));
            sim_settings::save_clock(
                &state.db,
                state.clock.now(),
                settings.multiplier,
                settings.paused,
                now,
            )
            .await?;
        }
        _ => {}
    }

    Ok(simulator_page::render_fragment(&state)
        .await?
        .into_response())
}

#[derive(Deserialize)]
pub struct SimSettingsForm {
    pub latency_ms: i64,
    pub failure_rate: f64,
}

/// `POST /sim/settings` — the global latency/failure sliders
/// (specs/008-Simulation.md item 2).
pub async fn set_sim_settings(
    State(state): State<AppState>,
    Form(body): Form<SimSettingsForm>,
) -> Result<Response, AppError> {
    let now = state.clock.now();
    sim_settings::save_latency_and_failure(
        &state.db,
        body.latency_ms.max(0),
        body.failure_rate.clamp(0.0, 1.0),
        now,
    )
    .await?;
    Ok(simulator_page::render_fragment(&state)
        .await?
        .into_response())
}

#[derive(Deserialize, Default)]
pub struct CreateFaultForm {
    pub provider_slug: Option<String>,
    pub method: Option<String>,
    pub path_glob: String,
    pub mode: String,
    pub http_status: Option<i32>,
    pub error_code: Option<String>,
    pub latency_ms: Option<i64>,
    pub probability: Option<f64>,
    pub remaining: Option<i32>,
    pub note: Option<String>,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

/// `POST /sim/faults` — the Simulator page's fault-rule form, a thin
/// client of the same `db::repo::faults` the `/admin/faults` API uses
/// (specs/008-Simulation.md item 4).
pub async fn create_sim_fault(
    State(state): State<AppState>,
    Form(body): Form<CreateFaultForm>,
) -> Result<Response, AppError> {
    let now = state.clock.now();
    faults::create(
        &state.db,
        &faults::NewFault {
            provider_slug: non_empty(body.provider_slug),
            method: non_empty(body.method),
            path_glob: body.path_glob,
            mode: body.mode,
            http_status: body.http_status,
            error_code: non_empty(body.error_code),
            latency_ms: body.latency_ms,
            probability: body.probability.unwrap_or(1.0),
            remaining: body.remaining,
            note: non_empty(body.note),
            created_at: now,
            expires_at: None,
        },
    )
    .await?;

    Ok(simulator_page::render_fragment(&state)
        .await?
        .into_response())
}

pub async fn delete_sim_fault(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    if let Ok(id) = id.parse::<FaultId>() {
        faults::delete(&state.db, id).await?;
    }
    Ok(simulator_page::render_fragment(&state)
        .await?
        .into_response())
}

/// `POST /sim/reset` — "Reset and reseed," trimmed to a real reset (no
/// Faker-driven scale generation until M7, specs/008-Simulation.md's own
/// Caveat).
pub async fn reset_sim(State(state): State<AppState>) -> Result<Response, AppError> {
    sim_settings::reset_dynamic_tables(&state.db).await?;
    crate::db::bootstrap_demo_credentials(&state.db).await?;
    Ok(simulator_page::render_fragment(&state)
        .await?
        .into_response())
}

/// `POST /providers/acmepay/payments` — the `/providers/acmepay` page's
/// "Create a payment" form, calling `acmepay::routes::resolve_and_charge`
/// directly (the demo merchant, not a bearer token, identifies who the
/// payment belongs to — see design.md's "Direct domain call" decision in
/// `openspec/changes/provider-create-forms`).
pub async fn create_payment_from_provider_page(
    State(state): State<AppState>,
    Form(body): Form<providers_page::CreatePaymentTryForm>,
) -> Result<Response, AppError> {
    let Some(credential) = providers::demo_credential(&state.db, "acmepay").await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let input = body.clone();

    let payment_method = PaymentMethodInput {
        kind: "card".to_string(),
        card: Some(CardInput {
            number: body.card_number,
            exp_month: body.exp_month,
            exp_year: body.exp_year,
            cvv: non_empty(body.cvv),
            holder: non_empty(body.holder),
        }),
        installments: None,
    };

    let outcome = acmepay_routes::resolve_and_charge(
        &state,
        acmepay_routes::ChargeInput {
            merchant_id: credential.merchant_id,
            amount: body.amount,
            currency: body.currency,
            capture_mode: None,
            reference: non_empty(body.reference),
            payment_method: &payment_method,
            customer_email: None,
            metadata: None,
            explicit_scenario: None,
        },
    )
    .await;

    let outcome = match outcome {
        Ok(outcome) => {
            let row = payments::get(&state.db, outcome.payment_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("payment vanished mid-mutation"))?;
            let (tone, label) = components::payment_tone(row.status);
            providers_page::PaymentTryOutcome::Success(providers_page::PaymentTryResult {
                id: row.id.to_string(),
                tone,
                label: label.to_string(),
                amount_cents: row.amount_cents,
                currency: row.currency,
            })
        }
        Err(err) => providers_page::PaymentTryOutcome::Error {
            input,
            message: acme_error_message(err)?,
        },
    };

    Ok(providers_page::payment_try_fragment(Some(outcome)).into_response())
}

/// `POST /providers/acmepay/webhook_endpoints` — the same page's "Register
/// a webhook endpoint" form, mirroring
/// `acmepay::routes::create_webhook_endpoint`'s own secret generation and
/// row creation.
/// Blank input, or an explicit `*`, both mean "all events" and must be
/// stored as `["*"]` — `db::repo::webhooks::fan_out`'s match only checks
/// for a literal `"*"` element, so storing `[]` (the previous behavior)
/// silently matched nothing instead of everything.
fn parse_enabled_events(raw: Option<String>) -> Vec<String> {
    let trimmed = raw.as_deref().unwrap_or("").trim();
    if trimmed.is_empty() || trimmed == "*" {
        return vec!["*".to_string()];
    }
    trimmed
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .collect()
}

pub async fn create_webhook_endpoint_from_provider_page(
    State(state): State<AppState>,
    Form(body): Form<providers_page::CreateWebhookTryForm>,
) -> Result<Response, AppError> {
    let Some(credential) = providers::demo_credential(&state.db, "acmepay").await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    let events = parse_enabled_events(body.enabled_events);
    let now = state.clock.now();
    let secret = format!("whsec_test_{}", ulid::Ulid::new());

    let id = webhooks::create(
        &state.db,
        &webhooks::NewWebhookEndpoint {
            merchant_id: credential.merchant_id,
            provider_slug: "acmepay".to_string(),
            url: body.url.clone(),
            secret: secret.clone(),
            enabled_events: serde_json::to_string(&events)?,
            description: non_empty(body.description),
            created_at: now,
        },
    )
    .await?;

    let result = Ok(providers_page::WebhookTryResult {
        id: id.to_string(),
        url: body.url,
        events,
        secret,
    });

    Ok(providers_page::webhook_try_fragment(Some(result)).into_response())
}

/// `POST /providers/acmeship/shipments` — the `/providers/acmeship` page's
/// "Create a shipment" form, booking directly through
/// `acmeship::routes::book_shipment_direct` (no separate rate-quote step).
pub async fn create_shipment_from_provider_page(
    State(state): State<AppState>,
    Form(body): Form<providers_page::CreateShipmentTryForm>,
) -> Result<Response, AppError> {
    let Some(credential) = providers::demo_credential(&state.db, "acmeship").await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let input = body.clone();

    let origin = AddressInput {
        postal_code: body.origin_postal_code,
        city: None,
        country: body.origin_country,
        residential: None,
    };
    let destination = AddressInput {
        postal_code: body.destination_postal_code,
        city: None,
        country: body.destination_country,
        residential: None,
    };
    let packages = vec![PackageInput {
        weight_grams: body.weight_grams,
        length_cm: body.length_cm,
        width_cm: body.width_cm,
        height_cm: body.height_cm,
    }];
    let declared_value = MoneyInput {
        amount: body.declared_value_amount,
        currency: body.declared_value_currency,
    };
    let order_reference = non_empty(body.order_reference);
    let recipient_name = non_empty(body.recipient_name);

    let outcome = acmeship_routes::book_shipment_direct(
        &state,
        acmeship_routes::BookShipmentInput {
            merchant_id: credential.merchant_id,
            origin: Some(&origin),
            destination: Some(&destination),
            packages: Some(&packages),
            declared_value: Some(&declared_value),
            service_code: body.service_code.as_deref(),
            order_reference: order_reference.as_deref(),
            recipient_name: recipient_name.as_deref(),
            explicit_scenario: None,
        },
    )
    .await;

    let outcome = match outcome {
        Ok(id) => {
            let row = shipments::get(&state.db, id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("shipment vanished mid-mutation"))?;
            let (tone, label) = components::shipment_tone(row.status);
            providers_page::ShipmentTryOutcome::Success(providers_page::ShipmentTryResult {
                id: row.id.to_string(),
                tracking_number: row.tracking_number.clone(),
                tone,
                label: label.to_string(),
                price_cents: row.price_cents,
                currency: row.currency,
            })
        }
        Err(err) => providers_page::ShipmentTryOutcome::Error {
            input,
            message: acme_error_message(err)?,
        },
    };

    Ok(providers_page::shipment_try_fragment(Some(outcome)).into_response())
}
