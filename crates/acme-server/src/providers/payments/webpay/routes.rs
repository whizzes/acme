//! Trancorp Webpay handlers (spec §10.2). Dialect in, dialect out: every DB
//! access goes through `db::repo`, never `sqlx::query*` directly (spec §5).
//!
//! Trimmed from spec §10.2's full route table (specs/006-Dialects.md item
//! 2): `PUT .../capture` (deferred capture) and the `/rswebpaytransaction/
//! api/webpay/v1.0/transactions` mall variant are not implemented this
//! slice — the create→commit→status→refund lifecycle is.

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::db::repo::payments::{self, CheckoutSessionRow, NewCheckoutSession, NewRefund};
use crate::domain::ids::CheckoutSessionId;
use crate::domain::money::{Currency, Money};
use crate::domain::payment::{self, PaymentCommand, PaymentStatus};
use crate::domain::scenario::{self, PaymentInputs, PaymentScenario};
use crate::http::auth::AuthenticatedMerchant;
use crate::providers::payments::webpay::dto::*;
use crate::providers::payments::webpay::map;
use crate::state::AppState;

/// Trancorp's own error envelope (spec §10.2/§10.6: `error_message`, not
/// the Stripe-shaped envelope the reference dialects use).
pub enum WebpayError {
    NotFound,
    /// "Transaction already locked" — committing a session that isn't
    /// `open` anymore.
    Locked,
    Expired,
    Validation(String),
    Internal(anyhow::Error),
}

impl IntoResponse for WebpayError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            WebpayError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            WebpayError::Locked => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Transaction already locked".to_string(),
            ),
            WebpayError::Expired => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Token has expired".to_string(),
            ),
            WebpayError::Validation(msg) => (StatusCode::BAD_REQUEST, msg),
            WebpayError::Internal(error) => {
                tracing::error!(?error, "webpay: internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
        };
        (
            status,
            Json(WebpayErrorBody {
                error_message: message,
            }),
        )
            .into_response()
    }
}

impl From<anyhow::Error> for WebpayError {
    fn from(err: anyhow::Error) -> Self {
        WebpayError::Internal(err)
    }
}

impl From<crate::domain::error::DomainError> for WebpayError {
    fn from(err: crate::domain::error::DomainError) -> Self {
        WebpayError::Internal(err.into())
    }
}

impl From<serde_json::Error> for WebpayError {
    fn from(err: serde_json::Error) -> Self {
        WebpayError::Internal(err.into())
    }
}

fn explicit_scenario(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-acme-scenario")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// `checkout_sessions.line_items` repurposed to carry `{buy_order,
/// session_id}` — Trancorp's own request fields, which the shared
/// `checkout_sessions` table (built for Acme Pay's hosted checkout, spec
/// §10.1) has no dedicated columns for. See `db::repo::payments::
/// set_checkout_session_payment_status`'s doc comment for the matching
/// `payment_status` repurposing.
#[derive(serde::Serialize, serde::Deserialize)]
struct WebpayOrderRef {
    buy_order: String,
    session_id: String,
}

fn order_ref(session: &CheckoutSessionRow) -> WebpayOrderRef {
    session
        .line_items
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(WebpayOrderRef {
            buy_order: String::new(),
            session_id: String::new(),
        })
}

#[utoipa::path(
    post, path = "/transactions", operation_id = "create_transaction", tag = "transactions",
    description = "Create a transaction: returns a token and the hosted card-entry page to redirect the shopper to (spec §10.2).",
    request_body(content = CreateTransactionRequest, description = "Transaction to create"),
    responses((status = 200, description = "Transaction created", body = CreateTransactionResponse)),
    security(("tbk_key_pair" = []))
)]
pub async fn create_transaction(
    State(state): State<AppState>,
    Extension(AuthenticatedMerchant(merchant_id)): Extension<AuthenticatedMerchant>,
    headers: HeaderMap,
    Json(body): Json<CreateTransactionRequest>,
) -> Result<Json<CreateTransactionResponse>, WebpayError> {
    let explicit = explicit_scenario(&headers);
    // Trancorp never receives card data through the merchant API (spec
    // §10.2 — entry happens purely on the hosted page), so only the
    // amount-suffix magic values (spec §9.1) and an explicit override are
    // reachable here; PAN/email rules simply never match with both `None`.
    let scenario = scenario::resolve_payment_scenario(&PaymentInputs {
        pan: None,
        amount_cents: body.amount,
        email: None,
        method_kind: "card",
        explicit: explicit.as_deref(),
    })
    .map_err(|e| WebpayError::Validation(e.to_string()))?;

    let now = state.clock.now();
    let id = CheckoutSessionId::new();
    let expires_at = now + chrono::Duration::minutes(5);
    let hosted_url = format!("{}/webpay/checkout", state.cfg.public_url);
    let order_ref = serde_json::to_string(&WebpayOrderRef {
        buy_order: body.buy_order.clone(),
        session_id: body.session_id.clone(),
    })?;

    payments::create_checkout_session(
        &state.db,
        id,
        &NewCheckoutSession {
            merchant_id,
            provider_slug: "webpay".to_string(),
            amount_cents: body.amount,
            currency: "CLP".to_string(),
            line_items: Some(order_ref),
            customer_email: None,
            success_url: Some(body.return_url.clone()),
            cancel_url: None,
            created_at: now,
            expires_at,
            hosted_url: hosted_url.clone(),
        },
    )
    .await?;
    payments::set_checkout_session_payment_status(&state.db, id, scenario.as_str(), now).await?;

    Ok(Json(CreateTransactionResponse {
        token: id.to_string(),
        url: hosted_url,
    }))
}

#[utoipa::path(
    put, path = "/transactions/{token}", operation_id = "commit_transaction", tag = "transactions",
    description = "Commit a transaction after the shopper returns from the hosted page (spec §10.2).",
    params(("token" = String, Path, description = "Transaction token from create")),
    responses(
        (status = 200, description = "Transaction committed", body = TransactionDetail),
        (status = 404, description = "No transaction with that token", body = WebpayErrorBody),
        (status = 422, description = "Already committed, or the token has expired", body = WebpayErrorBody),
    ),
    security(("tbk_key_pair" = []))
)]
pub async fn commit_transaction(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<TransactionDetail>, WebpayError> {
    let id: CheckoutSessionId = token.parse().map_err(|_| WebpayError::NotFound)?;
    let session = payments::get_checkout_session(&state.db, id)
        .await?
        .ok_or(WebpayError::NotFound)?;

    let now = state.clock.now();
    if session.status == "closed" {
        return Err(WebpayError::Locked);
    }
    if session.status == "expired" || session.expires_at < now {
        if session.status != "expired" {
            payments::expire_checkout_session(&state.db, id, now).await?;
        }
        return Err(WebpayError::Expired);
    }

    let refs = order_ref(&session);
    let scenario = PaymentScenario::from_str_or_approve(&session.payment_status);

    let payment_id = payments::create(
        &state.db,
        &payments::NewPayment {
            merchant_id: session.merchant_id,
            provider_slug: "webpay".to_string(),
            amount_cents: session.amount_cents,
            currency: session.currency.clone(),
            capture_mode: "automatic".to_string(),
            reference: Some(refs.buy_order.clone()),
            method_kind: "card".to_string(),
            method_detail: None,
            installments: 1,
            scenario: scenario.as_str().to_string(),
            risk_score: None,
            risk_decision: None,
            three_ds: None,
            metadata: None,
            created_at: now,
        },
    )
    .await?;

    let mut status = PaymentStatus::Created;
    let mut seq = payments::event_count(&state.db, payment_id).await?;
    for cmd in scenario::payment_initial_steps(scenario) {
        let transition = payment::apply(status, cmd.clone(), &state.clock)
            .expect("scenario-derived initial steps are always legal transitions from Created");
        let status_reason = match &cmd {
            PaymentCommand::Reject(reason) => Some(reason.clone().as_str()),
            _ => None,
        };
        payments::advance(
            &state.db,
            payment_id,
            session.merchant_id,
            "webpay",
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

    // `manual_review` (amount ending `51`, spec §9.1) only reaches
    // `Pending` synchronously — the delayed `Authorize`/`Capture` hop is
    // the ticker's job, exactly as it is for `acmepay::create_payment`.
    if let Some((delay, _)) = scenario::payment_next_step(scenario, status) {
        payments::schedule_next(&state.db, payment_id, Some(now + delay)).await?;
    }

    payments::close_checkout_session(
        &state.db,
        id,
        payment_id,
        if status == PaymentStatus::Rejected {
            "unpaid"
        } else {
            "paid"
        },
        now,
    )
    .await?;

    let row = payments::get(&state.db, payment_id)
        .await?
        .ok_or_else(|| WebpayError::Internal(anyhow::anyhow!("payment vanished mid-commit")))?;

    Ok(Json(map::transaction_detail(
        row.status,
        refs.buy_order,
        refs.session_id,
        row.amount_cents,
        &token,
        row.captured_at,
        now,
    )))
}

#[utoipa::path(
    get, path = "/transactions/{token}", operation_id = "get_transaction_status", tag = "transactions",
    description = "Retrieve a transaction's current status by token, without committing it.",
    params(("token" = String, Path, description = "Transaction token from create")),
    responses(
        (status = 200, description = "Transaction status", body = TransactionDetail),
        (status = 404, description = "No transaction with that token", body = WebpayErrorBody),
    ),
    security(("tbk_key_pair" = []))
)]
pub async fn get_transaction_status(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<TransactionDetail>, WebpayError> {
    let id: CheckoutSessionId = token.parse().map_err(|_| WebpayError::NotFound)?;
    let session = payments::get_checkout_session(&state.db, id)
        .await?
        .ok_or(WebpayError::NotFound)?;
    let refs = order_ref(&session);
    let now = state.clock.now();

    if let Some(payment_id) = session.payment_id {
        let row = payments::get(&state.db, payment_id)
            .await?
            .ok_or(WebpayError::NotFound)?;
        return Ok(Json(map::transaction_detail(
            row.status,
            refs.buy_order,
            refs.session_id,
            row.amount_cents,
            &token,
            row.captured_at,
            now,
        )));
    }

    // Not committed yet: `INITIALIZED`, no authorization code, response
    // code `0` (nothing has been declined — nothing has been decided).
    Ok(Json(map::transaction_detail(
        PaymentStatus::Created,
        refs.buy_order,
        refs.session_id,
        session.amount_cents,
        &token,
        None,
        now,
    )))
}

#[utoipa::path(
    post, path = "/transactions/{token}/refunds", operation_id = "refund_transaction", tag = "transactions",
    description = "Refund or nullify a committed, captured transaction.",
    params(("token" = String, Path, description = "Transaction token from create")),
    request_body(content = RefundRequest, description = "Refund options"),
    responses(
        (status = 200, description = "Refund or nullification applied", body = RefundResponse),
        (status = 404, description = "No transaction with that token", body = WebpayErrorBody),
        (status = 422, description = "Transaction is not in a refundable state", body = WebpayErrorBody),
    ),
    security(("tbk_key_pair" = []))
)]
pub async fn refund_transaction(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(body): Json<RefundRequest>,
) -> Result<Json<RefundResponse>, WebpayError> {
    let id: CheckoutSessionId = token.parse().map_err(|_| WebpayError::NotFound)?;
    let session = payments::get_checkout_session(&state.db, id)
        .await?
        .ok_or(WebpayError::NotFound)?;
    let payment_id = session.payment_id.ok_or(WebpayError::Locked)?;

    let row = payments::get(&state.db, payment_id)
        .await?
        .ok_or(WebpayError::NotFound)?;
    if !matches!(
        row.status,
        PaymentStatus::Captured | PaymentStatus::PartiallyRefunded
    ) {
        return Err(WebpayError::Validation(
            "transaction is not in a refundable state".to_string(),
        ));
    }

    let now = state.clock.now();
    let currency: Currency = row
        .currency
        .parse()
        .map_err(|_| WebpayError::Internal(anyhow::anyhow!("unknown stored currency")))?;
    let remaining_before = row.amount_cents - row.amount_refunded_cents;
    let refund_amount = body.amount.unwrap_or(remaining_before);
    if refund_amount <= 0 || refund_amount > remaining_before {
        return Err(WebpayError::Validation(
            "amount exceeds the refundable balance".to_string(),
        ));
    }
    let remaining_after = remaining_before - refund_amount;
    let seq = payments::event_count(&state.db, payment_id).await?;

    if row.status == PaymentStatus::Captured {
        let transition = payment::apply(
            PaymentStatus::Captured,
            PaymentCommand::RefundPartially {
                amount: Money::new(refund_amount, currency),
            },
            &state.clock,
        )?;
        payments::advance(
            &state.db,
            payment_id,
            row.merchant_id,
            "webpay",
            transition.to,
            transition.event,
            seq,
            transition.occurred_at,
            None,
            None,
        )
        .await?;
        if remaining_after == 0 {
            let transition2 = payment::apply(transition.to, PaymentCommand::Refund, &state.clock)?;
            payments::advance(
                &state.db,
                payment_id,
                row.merchant_id,
                "webpay",
                transition2.to,
                transition2.event,
                seq + 1,
                transition2.occurred_at,
                None,
                None,
            )
            .await?;
        }
    } else if remaining_after == 0 {
        let transition = payment::apply(
            PaymentStatus::PartiallyRefunded,
            PaymentCommand::Refund,
            &state.clock,
        )?;
        payments::advance(
            &state.db,
            payment_id,
            row.merchant_id,
            "webpay",
            transition.to,
            transition.event,
            seq,
            transition.occurred_at,
            None,
            None,
        )
        .await?;
    }

    payments::create_refund(
        &state.db,
        &NewRefund {
            payment_id,
            amount_cents: refund_amount,
            reason: None,
            created_at: now,
        },
        row.merchant_id,
        "webpay",
    )
    .await?;

    Ok(Json(RefundResponse {
        kind: if remaining_after == 0 {
            "nullified".to_string()
        } else {
            "reversed".to_string()
        },
        amount: refund_amount,
        response_code: 0,
    }))
}
