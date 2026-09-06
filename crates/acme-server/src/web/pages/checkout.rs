//! Trancorp Webpay's hosted card-entry page (spec §13.7, specs/006-Dialects.md
//! item 4) — `/webpay/checkout?token_ws=…`. Deliberately styled unlike the
//! dashboard (spec §13.7: "so screenshots are never confused") and carries
//! the persistent *"Simulated checkout"* banner.

use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse};
use maud::{Markup, html};
use serde::Deserialize;

use crate::db::repo::payments;
use crate::domain::ids::CheckoutSessionId;
use crate::error::AppError;
use crate::state::AppState;

fn checkout_shell(body: Markup) -> Markup {
    html! {
        (maud::DOCTYPE)
        html lang="es" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Webpay" }
                link rel="stylesheet" href="/static/checkout.css";
            }
            body.webpay-checkout {
                div.webpay-banner { "Simulado — no ingreses datos de tarjeta reales." }
                main.webpay-card {
                    div.webpay-logo { "Webpay" }
                    (body)
                }
            }
        }
    }
}

#[derive(Deserialize)]
pub struct CheckoutQuery {
    pub token_ws: Option<String>,
}

pub async fn page(
    State(state): State<AppState>,
    Query(query): Query<CheckoutQuery>,
) -> Result<Markup, AppError> {
    let Some(token) = query.token_ws.filter(|t| !t.is_empty()) else {
        return Ok(checkout_shell(
            html! { p { "Falta el parámetro token_ws." } },
        ));
    };
    let Ok(id) = token.parse::<CheckoutSessionId>() else {
        return Ok(checkout_shell(html! { p { "Token inválido." } }));
    };
    let Some(session) = payments::get_checkout_session(&state.db, id).await? else {
        return Ok(checkout_shell(html! { p { "Transacción no encontrada." } }));
    };

    if session.status != "open" {
        return Ok(checkout_shell(
            html! { p { "Esta transacción ya no está disponible." } },
        ));
    }

    Ok(checkout_shell(html! {
        p.webpay-amount { (session.amount_cents) " " (session.currency.clone()) }
        form.webpay-card-form method="post" action="/webpay/checkout" {
            input type="hidden" name="token_ws" value=(token);
            label { "Número de tarjeta"
                input type="text" placeholder="4051 8856 0044 6623" readonly;
            }
            label { "Vencimiento"
                input type="text" placeholder="12/29" readonly;
            }
            label { "CVV"
                input type="text" placeholder="123" readonly;
            }
            div.webpay-actions {
                button type="submit" name="action" value="approve" { "Pagar (aprobar)" }
                button type="submit" name="action" value="reject" { "Forzar rechazo" }
                button type="submit" name="action" value="abandon" { "Abandonar" }
            }
        }
    }))
}

#[derive(Deserialize)]
pub struct SubmitForm {
    pub token_ws: String,
    pub action: String,
}

pub async fn submit(
    State(state): State<AppState>,
    axum::extract::Form(body): axum::extract::Form<SubmitForm>,
) -> Result<impl IntoResponse, AppError> {
    let Ok(id) = body.token_ws.parse::<CheckoutSessionId>() else {
        return Ok(Html(
            checkout_shell(html! { p { "Token inválido." } }).into_string(),
        ));
    };
    let Some(session) = payments::get_checkout_session(&state.db, id).await? else {
        return Ok(Html(
            checkout_shell(html! { p { "Transacción no encontrada." } }).into_string(),
        ));
    };
    if session.status != "open" {
        return Ok(Html(
            checkout_shell(html! { p { "Esta transacción ya no está disponible." } }).into_string(),
        ));
    }

    if body.action == "abandon" {
        return Ok(Html(
            checkout_shell(
                html! { p { "Has abandonado el pago. El comercio puede intentarlo de nuevo." } },
            )
            .into_string(),
        ));
    }

    let scenario = if body.action == "reject" {
        "decline_do_not_honor"
    } else {
        "approve"
    };
    let now = state.clock.now();
    payments::set_checkout_session_payment_status(&state.db, id, scenario, now).await?;

    let Some(return_url) = session.success_url else {
        return Ok(Html(
            checkout_shell(html! { p { "Listo. El comercio confirmará el pago en breve." } })
                .into_string(),
        ));
    };

    // Real Webpay redirects the shopper's browser back to `return_url` as
    // a form `POST` carrying `token_ws` (spec §10.2) — an auto-submitting
    // hidden form is the only way an HTTP response can make a browser
    // issue a cross-origin `POST` on its own.
    Ok(Html(
        html! {
            (maud::DOCTYPE)
            html {
                body {
                    form #return-form method="post" action=(return_url) {
                        input type="hidden" name="token_ws" value=(body.token_ws);
                    }
                    script { "document.getElementById('return-form').submit();" }
                    noscript {
                        p { "Pago procesado. " a href=(return_url) { "Continuar" } }
                    }
                }
            }
        }
        .into_string(),
    ))
}
