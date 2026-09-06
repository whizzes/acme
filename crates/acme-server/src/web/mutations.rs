//! Dashboard mutations (spec §13.3): advance/refund/exception. Each
//! returns the freshly re-rendered detail fragment, an HTMX outerHTML
//! swap of the whole detail body (spec §13.4 pattern 3, scaled up from
//! "swap one row" — a resource detail page has no row of its own).

use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::db::repo::payments::{self, NewRefund};
use crate::db::repo::shipments;
use crate::domain::ids::{PaymentId, ShipmentId};
use crate::domain::money::{Currency, Money};
use crate::domain::payment::{self, PaymentCommand, PaymentStatus};
use crate::domain::shipment::{self, ShipmentCommand};
use crate::error::AppError;
use crate::state::AppState;
use crate::web::pages::{payments as payments_page, shipments as shipments_page};

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
