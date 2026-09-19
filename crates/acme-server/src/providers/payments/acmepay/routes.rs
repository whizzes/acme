//! Acme Pay handlers (spec §10.1). Dialect in, dialect out: every DB access
//! goes through `db::repo`, never `sqlx::query*` directly (spec §5).

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;

use crate::db::repo::{events, payments, webhooks};
use crate::domain::ids::{CheckoutSessionId, EventId, PaymentId, WebhookEndpointId};
use crate::domain::money::{Currency, Money};
use crate::domain::payment::{self, PaymentCommand, PaymentStatus};
use crate::domain::scenario::{self, PaymentInputs, PaymentScenario};
use crate::error::{AcmeError, AcmeErrorBody, FieldError};
use crate::http::auth::AuthenticatedMerchant;
use crate::providers::payments::acmepay::dto::*;
use crate::providers::payments::acmepay::map;
use crate::state::AppState;

fn validation(param: &str, message: impl Into<String>) -> AcmeError {
    AcmeError::Validation(vec![FieldError {
        param: param.to_string(),
        message: message.into(),
    }])
}

fn explicit_scenario(
    headers: &HeaderMap,
    body_metadata: Option<&serde_json::Value>,
) -> Option<String> {
    headers
        .get("x-acme-scenario")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            body_metadata
                .and_then(|m| m.get("acme_scenario"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
}

/// Inputs `resolve_and_charge` needs, gathered from either a direct
/// `POST /payments` body or the hosted checkout form.
pub(crate) struct ChargeInput<'a> {
    pub merchant_id: crate::domain::ids::MerchantId,
    pub amount: i64,
    pub currency: String,
    pub capture_mode: Option<String>,
    pub reference: Option<String>,
    pub payment_method: &'a PaymentMethodInput,
    pub customer_email: Option<&'a str>,
    pub metadata: Option<&'a serde_json::Value>,
    pub explicit_scenario: Option<&'a str>,
}

pub(crate) struct ChargeOutcome {
    pub payment_id: PaymentId,
    pub status: PaymentStatus,
    pub decline_reason: Option<&'static str>,
}

/// Resolves a magic-value scenario for the given card/amount/email and
/// creates+advances the resulting payment (spec §9.1). The one place both
/// `create_payment` (the direct API) and the hosted checkout page
/// (`web::pages::acmepay_checkout`) run this logic, so the two entry
/// points can't silently diverge on which scenario a given input
/// resolves to. Returns `Ok` for a rejected payment too — a decline is a
/// normal outcome, not an error; callers decide how to surface it (the
/// API as a `402`, the checkout page as a redirect to `cancel_url`).
/// Returns `Err` only when no payment row was created at all (validation
/// failure or a short-circuit scenario).
pub(crate) async fn resolve_and_charge(
    state: &AppState,
    input: ChargeInput<'_>,
) -> Result<ChargeOutcome, AcmeError> {
    let pan = input
        .payment_method
        .card
        .as_ref()
        .map(|c| c.number.as_str());
    let method_kind = input.payment_method.kind.as_str();

    let scenario = scenario::resolve_payment_scenario(&PaymentInputs {
        pan,
        amount_cents: input.amount,
        email: input.customer_email,
        method_kind,
        explicit: input.explicit_scenario,
    })
    .map_err(|e| validation("metadata.acme_scenario", e.to_string()))?;

    if scenario::payment_scenario_short_circuits(scenario) {
        return Err(match scenario {
            PaymentScenario::ProcessingError => AcmeError::ProviderDown,
            PaymentScenario::ProviderError => {
                AcmeError::Internal(anyhow::anyhow!("simulated provider error"))
            }
            PaymentScenario::InvalidNumber => validation(
                "payment_method.card.number",
                "card number fails Luhn validation",
            ),
            _ => unreachable!("payment_scenario_short_circuits only returns these three variants"),
        });
    }

    if method_kind == "card" && input.payment_method.card.is_none() {
        return Err(validation(
            "payment_method.card",
            "required when payment_method.type is \"card\"",
        ));
    }

    let now = state.clock.now();
    let capture_mode = input
        .capture_mode
        .clone()
        .unwrap_or_else(|| "automatic".to_string());
    let installments = input.payment_method.installments.unwrap_or(1);

    let method_detail = if let Some(card) = &input.payment_method.card {
        let token = payments::store_card_token(
            &state.db,
            input.merchant_id,
            map::brand_from_pan(&card.number),
            &map::last4(&card.number),
            card.exp_month,
            card.exp_year,
            card.holder.as_deref(),
            scenario.as_str(),
            now,
        )
        .await?;
        Some(serde_json::to_string(&map::StoredCardDetail {
            brand: map::brand_from_pan(&card.number).to_string(),
            last4: map::last4(&card.number),
            exp_month: card.exp_month,
            exp_year: card.exp_year,
            token,
            installments,
        })?)
    } else {
        None
    };

    let (risk_decision, risk_score) = match scenario {
        PaymentScenario::RiskReject => (Some("reject".to_string()), Some(92)),
        PaymentScenario::ManualReview => (Some("review".to_string()), Some(55)),
        _ => (Some("approve".to_string()), Some(12)),
    };
    let three_ds = matches!(
        scenario,
        PaymentScenario::ThreeDsChallenge | PaymentScenario::ThreeDsFail
    )
    .then(|| "challenge_required".to_string());

    let id = payments::create(
        &state.db,
        &payments::NewPayment {
            merchant_id: input.merchant_id,
            provider_slug: "acmepay".to_string(),
            amount_cents: input.amount,
            currency: input.currency.clone(),
            capture_mode: capture_mode.clone(),
            reference: input.reference.clone(),
            method_kind: method_kind.to_string(),
            method_detail,
            installments,
            scenario: scenario.as_str().to_string(),
            risk_score: risk_score.map(i64::from),
            risk_decision,
            three_ds,
            metadata: input.metadata.map(|m| m.to_string()),
            created_at: now,
        },
    )
    .await?;

    let mut steps = scenario::payment_initial_steps(scenario);
    if capture_mode == "manual" {
        steps.retain(|c| !matches!(c, PaymentCommand::Capture));
    }

    let mut status = PaymentStatus::Created;
    let mut decline_reason: Option<&'static str> = None;
    let mut seq = payments::event_count(&state.db, id).await?;

    for cmd in steps {
        let transition = payment::apply(status, cmd.clone(), &state.clock)
            .expect("scenario-derived initial steps are always legal transitions from Created");
        let status_reason = match &cmd {
            PaymentCommand::Reject(reason) => {
                decline_reason = Some(reason.clone().as_str());
                decline_reason
            }
            _ => None,
        };
        payments::advance(
            &state.db,
            id,
            input.merchant_id,
            "acmepay",
            transition.to,
            transition.event,
            seq,
            transition.occurred_at,
            None,
            status_reason,
        )
        .await?;
        status = transition.to;
        seq += 1;
    }

    if let Some((delay, _)) = scenario::payment_next_step(scenario, status) {
        payments::schedule_next(&state.db, id, Some(now + delay)).await?;
    }

    Ok(ChargeOutcome {
        payment_id: id,
        status,
        decline_reason,
    })
}

#[utoipa::path(
    post, path = "/payments", operation_id = "create_payment", tag = "payments",
    description = "Create and, depending on the resolved scenario, immediately capture a payment (spec §9.1).",
    request_body(
        content = CreatePaymentRequest, description = "Payment to create",
        examples(
            ("approved" = (summary = "Card that will be approved", value = json!({
                "amount": 4599000, "currency": "CLP", "reference": "ORD-2026-000814",
                "payment_method": { "type": "card", "card": {
                    "number": "4111111111111111", "exp_month": 11, "exp_year": 2029,
                    "cvv": "123", "holder": "CAMILA FUENTES"
                } }
            }))),
            ("declined" = (summary = "Insufficient funds", value = json!({
                "amount": 4599000, "currency": "CLP",
                "payment_method": { "type": "card", "card": {
                    "number": "4000000000000002", "exp_month": 11, "exp_year": 2029
                } }
            }))),
        )
    ),
    params(("Acme-Idempotency-Key" = Option<String>, Header, nullable = false, description = "Client-supplied key; a repeated key with the same body replays the stored response instead of charging twice")),
    responses(
        (status = 201, description = "Payment created", body = Payment),
        (status = 400, description = "Request failed validation, e.g. an invalid card number", body = AcmeErrorBody),
        (status = 402, description = "Payment declined", body = AcmeErrorBody),
        (status = 409, description = "Idempotency key reused with a different request body, or a request with the same key is still in flight", body = AcmeErrorBody),
        (status = 500, description = "Simulated provider error (spec §9.1 magic value)", body = AcmeErrorBody),
        (status = 502, description = "Simulated processing error (spec §9.1 magic value)", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn create_payment(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    headers: HeaderMap,
    Json(body): Json<CreatePaymentRequest>,
) -> Result<(StatusCode, Json<Payment>), AcmeError> {
    let explicit = explicit_scenario(&headers, body.metadata.as_ref());
    let email = body.customer.as_ref().and_then(|c| c.email.as_deref());

    let outcome = resolve_and_charge(
        &state,
        ChargeInput {
            merchant_id,
            amount: body.amount,
            currency: body.currency.clone(),
            capture_mode: body.capture_mode.clone(),
            reference: body.reference.clone(),
            payment_method: &body.payment_method,
            customer_email: email,
            metadata: body.metadata.as_ref(),
            explicit_scenario: explicit.as_deref(),
        },
    )
    .await?;

    if outcome.status == PaymentStatus::Rejected {
        let code = outcome.decline_reason.unwrap_or("declined");
        return Err(AcmeError::Declined {
            code: code.to_string(),
            message: format!("payment declined: {code}"),
        });
    }

    let row = payments::get(&state.db, outcome.payment_id)
        .await?
        .ok_or_else(|| AcmeError::NotFound("payment"))?;
    Ok((StatusCode::CREATED, Json(map::payment_to_dto(&row))))
}

#[utoipa::path(
    get, path = "/payments/{id}", operation_id = "get_payment", tag = "payments",
    description = "Retrieve a payment by id.",
    params(("id" = String, Path, description = "Payment id, e.g. `pay_01J…`")),
    responses(
        (status = 200, description = "Payment retrieved", body = Payment),
        (status = 404, description = "No payment with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn get_payment(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<Payment>, AcmeError> {
    let id: PaymentId = id.parse().map_err(|_| AcmeError::NotFound("payment"))?;
    let row = payments::get(&state.db, id)
        .await?
        .filter(|r| r.merchant_id == merchant_id)
        .ok_or(AcmeError::NotFound("payment"))?;
    Ok(Json(map::payment_to_dto(&row)))
}

#[utoipa::path(
    get, path = "/payments", operation_id = "list_payments", tag = "payments",
    description = "List payments for the authenticated merchant, newest first.",
    params(ListPaymentsQuery),
    responses((status = 200, description = "Page of payments, newest first", body = PaymentList)),
    security(("bearer_token" = []))
)]
pub async fn list_payments(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Query(query): Query<ListPaymentsQuery>,
) -> Result<Json<PaymentList>, AcmeError> {
    let limit = query.limit.unwrap_or(10).clamp(1, 100);
    let starting_after = query.starting_after.as_deref().and_then(|s| s.parse().ok());
    let created_after = query
        .created_after
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    let status: Option<PaymentStatus> = query
        .status
        .as_deref()
        .map(|s| serde_json::from_value(serde_json::Value::String(s.to_string())))
        .transpose()
        .map_err(|_| validation("status", "unknown payment status"))?;

    let rows = payments::list(
        &state.db,
        &payments::ListFilter {
            merchant_id: Some(merchant_id),
            provider_slug: Some("acmepay".to_string()),
            status,
            created_after,
            search: None,
            limit: limit + 1,
            starting_after,
        },
    )
    .await?;

    let has_more = rows.len() as i64 > limit;
    let mut data: Vec<Payment> = rows
        .iter()
        .take(limit as usize)
        .map(map::payment_to_dto)
        .collect();
    let next_cursor = has_more
        .then(|| data.last().map(|p| p.id.clone()))
        .flatten();
    data.truncate(limit as usize);

    Ok(Json(PaymentList {
        object: "list".to_string(),
        url: "/acmepay/v1/payments".to_string(),
        has_more,
        data,
        next_cursor,
    }))
}

#[utoipa::path(
    post, path = "/payments/{id}/capture", operation_id = "capture_payment", tag = "payments",
    description = "Capture a payment that is currently authorized (manual capture mode).",
    params(("id" = String, Path, description = "Payment id")),
    request_body(content = CaptureRequest, description = "Capture options"),
    responses(
        (status = 200, description = "Payment captured", body = Payment),
        (status = 404, description = "No payment with that id", body = AcmeErrorBody),
        (status = 409, description = "Payment is not in a capturable state", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn capture_payment(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
    Json(_body): Json<CaptureRequest>,
) -> Result<Json<Payment>, AcmeError> {
    let id: PaymentId = id.parse().map_err(|_| AcmeError::NotFound("payment"))?;
    let row = payments::get(&state.db, id)
        .await?
        .filter(|r| r.merchant_id == merchant_id)
        .ok_or(AcmeError::NotFound("payment"))?;

    if row.status != PaymentStatus::Authorized {
        return Err(AcmeError::UnprocessableState {
            from: format!("{:?}", row.status).to_lowercase(),
            to: "captured".to_string(),
        });
    }

    let transition = payment::apply(row.status, PaymentCommand::Capture, &state.clock)?;
    let seq = payments::event_count(&state.db, id).await?;
    payments::advance(
        &state.db,
        id,
        merchant_id,
        "acmepay",
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
        .ok_or(AcmeError::NotFound("payment"))?;
    Ok(Json(map::payment_to_dto(&row)))
}

#[utoipa::path(
    post, path = "/payments/{id}/cancel", operation_id = "cancel_payment", tag = "payments",
    description = "Cancel a payment before it is captured.",
    params(("id" = String, Path, description = "Payment id")),
    responses(
        (status = 200, description = "Payment cancelled", body = Payment),
        (status = 404, description = "No payment with that id", body = AcmeErrorBody),
        (status = 409, description = "Payment is not cancellable", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn cancel_payment(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<Payment>, AcmeError> {
    let id: PaymentId = id.parse().map_err(|_| AcmeError::NotFound("payment"))?;
    let row = payments::get(&state.db, id)
        .await?
        .filter(|r| r.merchant_id == merchant_id)
        .ok_or(AcmeError::NotFound("payment"))?;

    if !matches!(
        row.status,
        PaymentStatus::Pending | PaymentStatus::Authorized
    ) {
        return Err(AcmeError::UnprocessableState {
            from: format!("{:?}", row.status).to_lowercase(),
            to: "cancelled".to_string(),
        });
    }

    let transition = payment::apply(row.status, PaymentCommand::Cancel, &state.clock)?;
    let seq = payments::event_count(&state.db, id).await?;
    payments::advance(
        &state.db,
        id,
        merchant_id,
        "acmepay",
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
        .ok_or(AcmeError::NotFound("payment"))?;
    Ok(Json(map::payment_to_dto(&row)))
}

#[utoipa::path(
    post, path = "/payments/{id}/refunds", operation_id = "create_refund", tag = "payments",
    description = "Refund a captured payment, in full or in part.",
    params(("id" = String, Path, description = "Payment id")),
    request_body(content = RefundRequest, description = "Refund to create"),
    responses(
        (status = 201, description = "Refund created", body = Refund),
        (status = 404, description = "No payment with that id", body = AcmeErrorBody),
        (status = 409, description = "Payment cannot be refunded further right now", body = AcmeErrorBody),
        (status = 400, description = "Refund amount exceeds the refundable balance", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn create_refund(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
    Json(body): Json<RefundRequest>,
) -> Result<(StatusCode, Json<Refund>), AcmeError> {
    let id: PaymentId = id.parse().map_err(|_| AcmeError::NotFound("payment"))?;
    let row = payments::get(&state.db, id)
        .await?
        .filter(|r| r.merchant_id == merchant_id)
        .ok_or(AcmeError::NotFound("payment"))?;

    let remaining_before = row.amount_cents - row.amount_refunded_cents;
    let refund_amount = body.amount.unwrap_or(remaining_before);
    if refund_amount <= 0 || refund_amount > remaining_before {
        return Err(validation(
            "amount",
            "must be positive and no more than the refundable balance",
        ));
    }

    let now = state.clock.now();
    let currency: Currency = row
        .currency
        .parse()
        .map_err(|_| AcmeError::Internal(anyhow::anyhow!("unknown stored currency")))?;
    let remaining_after = remaining_before - refund_amount;

    match row.status {
        PaymentStatus::Captured => {
            let transition = payment::apply(
                PaymentStatus::Captured,
                PaymentCommand::RefundPartially {
                    amount: Money::new(refund_amount, currency),
                },
                &state.clock,
            )?;
            let seq = payments::event_count(&state.db, id).await?;
            payments::advance(
                &state.db,
                id,
                merchant_id,
                "acmepay",
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
                    merchant_id,
                    "acmepay",
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
            let seq = payments::event_count(&state.db, id).await?;
            payments::advance(
                &state.db,
                id,
                merchant_id,
                "acmepay",
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
            // Another partial refund on an already-partially-refunded
            // payment has no further status transition to make (spec
            // §8.1's table has no `partially_refunded -> partially_refunded`
            // edge) — only the refund row and the running total change.
        }
        _ => {
            return Err(AcmeError::UnprocessableState {
                from: format!("{:?}", row.status).to_lowercase(),
                to: "refunded".to_string(),
            });
        }
    }

    let refund_id = payments::create_refund(
        &state.db,
        &payments::NewRefund {
            payment_id: id,
            amount_cents: refund_amount,
            reason: body.reason.clone(),
            created_at: now,
        },
        merchant_id,
        "acmepay",
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(Refund {
            id: refund_id.to_string(),
            payment_id: id.to_string(),
            amount: refund_amount,
            reason: body.reason,
            status: "succeeded".to_string(),
            created_at: now.to_rfc3339(),
        }),
    ))
}

#[utoipa::path(
    get, path = "/payments/{id}/refunds", operation_id = "list_refunds", tag = "payments",
    description = "List the refunds recorded against one payment.",
    params(("id" = String, Path, description = "Payment id")),
    responses((status = 200, description = "Refunds for this payment", body = RefundList)),
    security(("bearer_token" = []))
)]
pub async fn list_refunds(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<RefundList>, AcmeError> {
    let id: PaymentId = id.parse().map_err(|_| AcmeError::NotFound("payment"))?;
    payments::get(&state.db, id)
        .await?
        .filter(|r| r.merchant_id == merchant_id)
        .ok_or(AcmeError::NotFound("payment"))?;

    let rows = payments::list_refunds(&state.db, id).await?;
    let data = rows
        .into_iter()
        .map(|r| Refund {
            id: r.id.to_string(),
            payment_id: r.payment_id.to_string(),
            amount: r.amount_cents,
            reason: r.reason,
            status: r.status,
            created_at: r.created_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(RefundList {
        object: "list".to_string(),
        data,
    }))
}

#[utoipa::path(
    post, path = "/checkout/sessions", operation_id = "create_checkout_session", tag = "checkout",
    description = "Create a hosted checkout session.",
    request_body(content = CreateCheckoutSessionRequest, description = "Checkout session to create"),
    responses((status = 201, description = "Checkout session created", body = CheckoutSession)),
    security(("bearer_token" = []))
)]
pub async fn create_checkout_session(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Json(body): Json<CreateCheckoutSessionRequest>,
) -> Result<(StatusCode, Json<CheckoutSession>), AcmeError> {
    let id = CheckoutSessionId::new();
    let now = state.clock.now();
    let expires_at = now + chrono::Duration::minutes(30);
    let hosted_url = format!("{}/acmepay/c/{}", state.cfg.public_url, id);

    payments::create_checkout_session(
        &state.db,
        id,
        &payments::NewCheckoutSession {
            merchant_id,
            provider_slug: "acmepay".to_string(),
            amount_cents: body.amount,
            currency: body.currency.clone(),
            line_items: body.line_items.as_ref().map(|v| v.to_string()),
            customer_email: body.customer_email.clone(),
            success_url: body.success_url.clone(),
            cancel_url: body.cancel_url.clone(),
            created_at: now,
            expires_at,
            hosted_url: hosted_url.clone(),
        },
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(CheckoutSession {
            id: id.to_string(),
            object: "checkout_session".to_string(),
            status: "open".to_string(),
            payment_status: "unpaid".to_string(),
            payment_id: None,
            amount: body.amount,
            currency: body.currency,
            hosted_url,
            expires_at: expires_at.to_rfc3339(),
            created_at: now.to_rfc3339(),
        }),
    ))
}

fn checkout_session_dto(row: crate::db::repo::payments::CheckoutSessionRow) -> CheckoutSession {
    CheckoutSession {
        id: row.id.to_string(),
        object: "checkout_session".to_string(),
        status: row.status,
        payment_status: row.payment_status,
        payment_id: row.payment_id.map(|id| id.to_string()),
        amount: row.amount_cents,
        currency: row.currency,
        hosted_url: row.hosted_url,
        expires_at: row.expires_at.to_rfc3339(),
        created_at: row.created_at.to_rfc3339(),
    }
}

#[utoipa::path(
    get, path = "/checkout/sessions/{id}", operation_id = "get_checkout_session", tag = "checkout",
    description = "Retrieve a checkout session by id.",
    params(("id" = String, Path, description = "Checkout session id")),
    responses(
        (status = 200, description = "Checkout session retrieved", body = CheckoutSession),
        (status = 404, description = "No checkout session with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn get_checkout_session(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<CheckoutSession>, AcmeError> {
    let id: CheckoutSessionId = id
        .parse()
        .map_err(|_| AcmeError::NotFound("checkout session"))?;
    let row = payments::get_checkout_session(&state.db, id)
        .await?
        .filter(|r| r.merchant_id == merchant_id)
        .ok_or(AcmeError::NotFound("checkout session"))?;
    Ok(Json(checkout_session_dto(row)))
}

#[utoipa::path(
    post, path = "/checkout/sessions/{id}/expire", operation_id = "expire_checkout_session", tag = "checkout",
    description = "Expire a checkout session before its natural expiry.",
    params(("id" = String, Path, description = "Checkout session id")),
    responses(
        (status = 200, description = "Checkout session expired", body = CheckoutSession),
        (status = 404, description = "No checkout session with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn expire_checkout_session(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<CheckoutSession>, AcmeError> {
    let id: CheckoutSessionId = id
        .parse()
        .map_err(|_| AcmeError::NotFound("checkout session"))?;
    payments::get_checkout_session(&state.db, id)
        .await?
        .filter(|r| r.merchant_id == merchant_id)
        .ok_or(AcmeError::NotFound("checkout session"))?;

    payments::expire_checkout_session(&state.db, id, state.clock.now()).await?;

    let row = payments::get_checkout_session(&state.db, id)
        .await?
        .ok_or(AcmeError::NotFound("checkout session"))?;
    Ok(Json(checkout_session_dto(row)))
}

#[utoipa::path(
    get, path = "/events", operation_id = "list_events", tag = "events",
    description = "List this merchant's Acme Pay event log, newest first.",
    params(ListEventsQuery),
    responses((status = 200, description = "Page of this merchant's Acme Pay events, newest first", body = EventList)),
    security(("bearer_token" = []))
)]
pub async fn list_events(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Query(query): Query<ListEventsQuery>,
) -> Result<Json<EventList>, AcmeError> {
    let limit = query.limit.unwrap_or(10).clamp(1, 100);
    let starting_after: Option<EventId> =
        query.starting_after.as_deref().and_then(|s| s.parse().ok());

    let rows = events::list(&state.db, merchant_id, "acmepay", limit + 1, starting_after).await?;
    let has_more = rows.len() as i64 > limit;
    let mut data: Vec<EventLogEntry> = rows
        .iter()
        .take(limit as usize)
        .map(|r| EventLogEntry {
            id: r.id.to_string(),
            kind: r.kind.clone(),
            resource_type: r.resource_type.clone(),
            resource_id: r.resource_id.clone(),
            data: serde_json::from_str(&r.data).unwrap_or(serde_json::Value::Null),
            created_at: r.created_at.to_rfc3339(),
        })
        .collect();
    let next_cursor = has_more
        .then(|| data.last().map(|e| e.id.clone()))
        .flatten();
    data.truncate(limit as usize);

    Ok(Json(EventList {
        object: "list".to_string(),
        data,
        next_cursor,
    }))
}

#[utoipa::path(
    get, path = "/events/{id}", operation_id = "get_event", tag = "events",
    description = "Retrieve a single event log entry by id.",
    params(("id" = String, Path, description = "Event id")),
    responses(
        (status = 200, description = "Event retrieved", body = EventLogEntry),
        (status = 404, description = "No event with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn get_event(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<EventLogEntry>, AcmeError> {
    let id: EventId = id.parse().map_err(|_| AcmeError::NotFound("event"))?;
    let row = events::get(&state.db, merchant_id, id)
        .await?
        .ok_or(AcmeError::NotFound("event"))?;
    Ok(Json(EventLogEntry {
        id: row.id.to_string(),
        kind: row.kind,
        resource_type: row.resource_type,
        resource_id: row.resource_id,
        data: serde_json::from_str(&row.data).unwrap_or(serde_json::Value::Null),
        created_at: row.created_at.to_rfc3339(),
    }))
}

#[utoipa::path(
    post, path = "/webhook_endpoints", operation_id = "create_webhook_endpoint", tag = "webhook_endpoints",
    description = "Register a webhook endpoint. Matching events are signed and delivered by the background dispatcher.",
    request_body(content = CreateWebhookEndpointRequest, description = "Endpoint to register"),
    responses((status = 201, description = "Webhook endpoint registered", body = WebhookEndpoint)),
    security(("bearer_token" = []))
)]
pub async fn create_webhook_endpoint(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Json(body): Json<CreateWebhookEndpointRequest>,
) -> Result<(StatusCode, Json<WebhookEndpoint>), AcmeError> {
    let now = state.clock.now();
    let secret = format!("whsec_test_{}", ulid::Ulid::new());
    let enabled_events = serde_json::to_string(&body.enabled_events)?;

    let id = webhooks::create(
        &state.db,
        &webhooks::NewWebhookEndpoint {
            merchant_id,
            provider_slug: "acmepay".to_string(),
            url: body.url.clone(),
            secret,
            enabled_events,
            description: body.description.clone(),
            created_at: now,
        },
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(WebhookEndpoint {
            id: id.to_string(),
            url: body.url,
            enabled_events: body.enabled_events,
            active: true,
            description: body.description,
            created_at: now.to_rfc3339(),
        }),
    ))
}

fn webhook_endpoint_dto(row: crate::db::repo::webhooks::WebhookEndpointRow) -> WebhookEndpoint {
    WebhookEndpoint {
        id: row.id.to_string(),
        url: row.url,
        enabled_events: serde_json::from_str(&row.enabled_events).unwrap_or_default(),
        active: row.active,
        description: row.description,
        created_at: row.created_at.to_rfc3339(),
    }
}

#[utoipa::path(
    get, path = "/webhook_endpoints/{id}", operation_id = "get_webhook_endpoint", tag = "webhook_endpoints",
    description = "Retrieve a registered webhook endpoint by id.",
    params(("id" = String, Path, description = "Webhook endpoint id")),
    responses(
        (status = 200, description = "Webhook endpoint retrieved", body = WebhookEndpoint),
        (status = 404, description = "No webhook endpoint with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn get_webhook_endpoint(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<Json<WebhookEndpoint>, AcmeError> {
    let id: WebhookEndpointId = id
        .parse()
        .map_err(|_| AcmeError::NotFound("webhook endpoint"))?;
    let row = webhooks::get(&state.db, merchant_id, id)
        .await?
        .ok_or(AcmeError::NotFound("webhook endpoint"))?;
    Ok(Json(webhook_endpoint_dto(row)))
}

#[utoipa::path(
    delete, path = "/webhook_endpoints/{id}", operation_id = "delete_webhook_endpoint", tag = "webhook_endpoints",
    description = "Unregister a webhook endpoint.",
    params(("id" = String, Path, description = "Webhook endpoint id")),
    responses(
        (status = 204, description = "Webhook endpoint deleted"),
        (status = 404, description = "No webhook endpoint with that id", body = AcmeErrorBody),
    ),
    security(("bearer_token" = []))
)]
pub async fn delete_webhook_endpoint(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    Path(id): Path<String>,
) -> Result<StatusCode, AcmeError> {
    let id: WebhookEndpointId = id
        .parse()
        .map_err(|_| AcmeError::NotFound("webhook endpoint"))?;
    if webhooks::delete(&state.db, merchant_id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AcmeError::NotFound("webhook endpoint"))
    }
}

#[utoipa::path(
    get, path = "/payment_methods", operation_id = "list_payment_methods", tag = "payment_methods",
    description = "List the payment methods this provider supports.",
    responses((status = 200, description = "Payment methods this provider supports", body = PaymentMethodsResponse)),
    security(("bearer_token" = []))
)]
pub async fn list_payment_methods() -> Json<PaymentMethodsResponse> {
    Json(PaymentMethodsResponse {
        data: vec![
            PaymentMethodCatalogEntry {
                kind: "card".to_string(),
                label: "Credit or debit card".to_string(),
                countries: vec![
                    "CL".to_string(),
                    "BR".to_string(),
                    "MX".to_string(),
                    "AR".to_string(),
                    "ES".to_string(),
                ],
                currencies: vec![
                    "CLP".to_string(),
                    "BRL".to_string(),
                    "MXN".to_string(),
                    "ARS".to_string(),
                    "EUR".to_string(),
                    "USD".to_string(),
                ],
            },
            PaymentMethodCatalogEntry {
                kind: "pix".to_string(),
                label: "Pix instant transfer".to_string(),
                countries: vec!["BR".to_string()],
                currencies: vec!["BRL".to_string()],
            },
            PaymentMethodCatalogEntry {
                kind: "boleto".to_string(),
                label: "Boleto bancário".to_string(),
                countries: vec!["BR".to_string()],
                currencies: vec!["BRL".to_string()],
            },
        ],
    })
}
