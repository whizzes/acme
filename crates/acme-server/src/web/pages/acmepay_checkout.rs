//! Acme Pay's own hosted checkout page (spec §13.7,
//! specs/x7-Additional-Dialects.md item 1's planned `/acmepay/c/{cs_id}`
//! path) — replaces `web::mod::hosted_checkout_placeholder`'s `501` for
//! Acme Pay checkout sessions specifically. Unlike Trancorp Webpay's page
//! (`web::pages::checkout`), which only records a forced outcome for a
//! separate merchant-initiated commit call, this page resolves and closes
//! out the payment itself — Acme Pay's dialect has no external commit step
//! (spec §10.1: intent + capture).

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use maud::{Markup, html};
use serde::Deserialize;

use crate::db::repo::payments;
use crate::domain::ids::CheckoutSessionId;
use crate::domain::payment::PaymentStatus;
use crate::error::{AcmeError, AppError};
use crate::providers::payments::acmepay::dto::{CardInput, PaymentMethodInput};
use crate::providers::payments::acmepay::routes::{ChargeInput, resolve_and_charge};
use crate::state::AppState;
use crate::web::pages::components;

fn checkout_shell(body: Markup) -> Markup {
    html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Acme Pay Checkout" }
                link rel="stylesheet" href="/static/checkout.css";
            }
            body.acmepay-checkout {
                div.acmepay-banner { "Simulated checkout — do not enter real card details." }
                main.acmepay-card {
                    div.acmepay-logo { "Acme Pay" }
                    (body)
                }
            }
        }
    }
}

fn unavailable() -> Markup {
    checkout_shell(html! { p { "This checkout is no longer available." } })
}

fn not_found() -> Markup {
    checkout_shell(html! { p { "Checkout session not found." } })
}

async fn load_open_session(
    state: &AppState,
    cs_id: &str,
) -> Result<Result<crate::db::repo::payments::CheckoutSessionRow, Markup>, AppError> {
    let Ok(id) = cs_id.parse::<CheckoutSessionId>() else {
        return Ok(Err(not_found()));
    };
    let Some(session) = payments::get_checkout_session(&state.db, id).await? else {
        return Ok(Err(not_found()));
    };
    if session.status != "open" || session.expires_at <= state.clock.now() {
        return Ok(Err(unavailable()));
    }
    Ok(Ok(session))
}

pub async fn page(
    State(state): State<AppState>,
    Path(cs_id): Path<String>,
) -> Result<Markup, AppError> {
    let session = match load_open_session(&state, &cs_id).await? {
        Ok(session) => session,
        Err(page) => return Ok(page),
    };

    Ok(checkout_shell(html! {
        p.acmepay-amount { (components::money(session.amount_cents, &session.currency)) }
        form.acmepay-card-form method="post" action={ "/acmepay/c/" (cs_id) } {
            label { "Card number"
                input type="text" name="number" placeholder="4111 1111 1111 1111" required;
            }
            label { "Cardholder"
                input type="text" name="holder" placeholder="CAMILA FUENTES";
            }
            div.acmepay-row {
                label { "Exp. month"
                    input type="number" name="exp_month" min="1" max="12" placeholder="11" required;
                }
                label { "Exp. year"
                    input type="number" name="exp_year" min="2024" max="2099" placeholder="2029" required;
                }
                label { "CVV"
                    input type="text" name="cvv" placeholder="123";
                }
            }
            div.acmepay-actions {
                button type="submit" { "Pay" }
            }
        }
        p.acmepay-hint {
            "Use one of the "
            a href="/providers/acmepay" { "published test cards" }
            " to force an outcome."
        }
    }))
}

#[derive(Deserialize)]
pub struct SubmitForm {
    pub number: String,
    pub holder: Option<String>,
    pub exp_month: i32,
    pub exp_year: i32,
    pub cvv: Option<String>,
}

fn charge_error_message(error: &AcmeError) -> &'static str {
    match error {
        AcmeError::Validation(_) => {
            "That card number isn't one of the published test cards. Use a published test PAN."
        }
        _ => "Something went wrong processing this card. Please try again.",
    }
}

pub async fn submit(
    State(state): State<AppState>,
    Path(cs_id): Path<String>,
    axum::extract::Form(form): axum::extract::Form<SubmitForm>,
) -> Result<impl IntoResponse, AppError> {
    let session = match load_open_session(&state, &cs_id).await? {
        Ok(session) => session,
        Err(page) => return Ok(page.into_response()),
    };

    let payment_method = PaymentMethodInput {
        kind: "card".to_string(),
        card: Some(CardInput {
            number: form.number,
            exp_month: form.exp_month,
            exp_year: form.exp_year,
            cvv: form.cvv,
            holder: form.holder,
        }),
        installments: None,
    };

    let outcome = match resolve_and_charge(
        &state,
        ChargeInput {
            merchant_id: session.merchant_id,
            amount: session.amount_cents,
            currency: session.currency.clone(),
            capture_mode: None,
            reference: Some(session.id.to_string()),
            payment_method: &payment_method,
            customer_email: None,
            metadata: None,
            explicit_scenario: None,
        },
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            return Ok(checkout_shell(html! {
                p.acmepay-error { (charge_error_message(&error)) }
                a href={ "/acmepay/c/" (cs_id) } { "Try again" }
            })
            .into_response());
        }
    };

    let payment_status = if outcome.status == PaymentStatus::Captured {
        "paid"
    } else {
        "unpaid"
    };
    payments::close_checkout_session(
        &state.db,
        session.id,
        outcome.payment_id,
        payment_status,
        state.clock.now(),
    )
    .await?;

    let redirect_url = match outcome.status {
        PaymentStatus::Captured => session.success_url.as_deref(),
        _ => session.cancel_url.as_deref(),
    };

    match redirect_url {
        Some(url) => Ok(Redirect::to(url).into_response()),
        None if outcome.status == PaymentStatus::Captured => Ok(checkout_shell(html! {
            p { "Payment approved. The merchant will confirm shortly." }
        })
        .into_response()),
        None => Ok(checkout_shell(html! {
            p { "Payment declined." }
            a href={ "/acmepay/c/" (cs_id) } { "Try again" }
        })
        .into_response()),
    }
}
