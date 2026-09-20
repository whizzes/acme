//! `/providers` catalog + `/providers/{slug}` detail (spec §13.3,
//! specs/006-Dialects.md item 5).

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use maud::{Markup, html};
use serde::Deserialize;

use crate::db::repo::providers::{self, ProviderRow};
use crate::error::AppError;
use crate::state::AppState;
use crate::web::layout::{Ctx, NavItem, layout};
use crate::web::pages::components;

/// Every provider spec §10/§11 eventually add, shown as a placeholder row
/// until its own module exists — a running checklist for
/// specs/007-Additional-Dialects.md (specs/006-Dialects.md item 5's own
/// reasoning for shipping this page now rather than waiting for all ten).
const PLANNED: &[(&str, &str)] = &[
    ("pagorapido", "Pago Rápido"),
    ("chargeflow", "Chargeflow"),
    ("nordika", "Nordika Pay"),
    ("postalis", "Postalis"),
    ("shiphub", "ShipHub"),
    ("veloz", "Veloz"),
];

pub async fn list(State(state): State<AppState>) -> Result<Markup, AppError> {
    let clock_label = components::timestamp(state.clock.now());
    let rows = providers::list(&state.db).await?;
    let built: std::collections::HashSet<&str> = rows.iter().map(|r| r.slug.as_str()).collect();

    Ok(layout(
        &Ctx {
            title: "Providers",
            clock_label: &clock_label,
            active: NavItem::Providers,
        },
        html! {
            h1 { "Providers" }
            table.data {
                thead { tr { th { "Provider" } th { "Kind" } th { "Dialect" } th { "Auth" } th { "Base path" } th { "" } } }
                tbody {
                    @for row in &rows { (provider_row(row)) }
                    @for (slug, name) in PLANNED {
                        @if !built.contains(slug) { (planned_row(slug, name)) }
                    }
                }
            }
        },
    ))
}

fn provider_row(row: &ProviderRow) -> Markup {
    html! {
        tr {
            td { a href={ "/providers/" (row.slug) } { (row.display_name.clone()) } }
            td { (row.kind.clone()) }
            td { (row.dialect.clone()) }
            td.mono { (row.auth_scheme.clone()) }
            td.mono { (row.base_path.clone()) }
            td { (components::pill(if row.enabled { "ok" } else { "idle" }, if row.enabled { "Built" } else { "Disabled" })) }
        }
    }
}

fn planned_row(slug: &str, name: &str) -> Markup {
    html! {
        tr.provider-row--planned {
            td { (name) }
            td { "—" }
            td { "—" }
            td { "—" }
            td.mono { (slug) }
            td { (components::pill("idle", "Planned")) }
        }
    }
}

pub async fn detail(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let clock_label = components::timestamp(state.clock.now());
    let Some(provider) = providers::get(&state.db, &slug).await? else {
        return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
    };
    let credential = providers::demo_credential(&state.db, &slug).await?;

    Ok(layout(
        &Ctx {
            title: "Provider",
            clock_label: &clock_label,
            active: NavItem::Providers,
        },
        html! {
            h1 { (provider.display_name.clone()) }
            dl.kv {
                dt { "kind" } dd { (provider.kind.clone()) }
                dt { "dialect" } dd { (provider.dialect.clone()) }
                dt { "auth scheme" } dd.mono { (provider.auth_scheme.clone()) }
                dt { "base path" } dd.mono { (provider.base_path.clone()) }
            }

            @if let Some(cred) = &credential {
                h2 { "Sandbox credential" }
                p { "This is a sandbox — credentials are shown in full (spec §18)." }
                dl.kv {
                    @if let Some(public_key) = &cred.public_key {
                        dt { "public key / client id" } dd.mono { (public_key.clone()) }
                    }
                    dt { "secret key" } dd.mono { (cred.secret_key.clone()) }
                }
            }

            h2 { "Try it" }
            @match provider.slug.as_str() {
                "acmepay" => {
                    (payment_try_fragment(None))
                    (webhook_try_fragment(None))
                }
                "acmeship" => (shipment_try_fragment(None)),
                _ => {}
            }
            p {
                a href="/docs" { "Open in Swagger UI" }
                " · "
                a href={ "/providers/" (provider.slug) "/docs" } { "Docs" }
            }
        },
    )
    .into_response())
}

/// The `/providers/acmepay/payments` form body — also reused to repopulate
/// the form's inputs with what the merchant actually typed when their
/// submission fails validation, instead of resetting to placeholder
/// defaults (spec `provider-try-it-forms` - "Submitting invalid input").
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CreatePaymentTryForm {
    pub amount: i64,
    pub currency: String,
    pub card_number: String,
    pub exp_month: i32,
    pub exp_year: i32,
    pub cvv: Option<String>,
    pub holder: Option<String>,
    pub reference: Option<String>,
}

impl Default for CreatePaymentTryForm {
    fn default() -> Self {
        Self {
            amount: 4599,
            currency: "USD".to_string(),
            card_number: "4111111111111111".to_string(),
            exp_month: 11,
            exp_year: 2029,
            cvv: None,
            holder: None,
            reference: None,
        }
    }
}

/// A payment created through `payment_try_fragment`'s form.
pub struct PaymentTryResult {
    pub id: String,
    pub tone: &'static str,
    pub label: String,
    pub amount_cents: i64,
    pub currency: String,
}

/// Either outcome `payment_try_fragment` can render after a submission.
pub enum PaymentTryOutcome {
    Success(PaymentTryResult),
    /// Carries the rejected submission back so its fields can repopulate
    /// the redrawn form instead of losing what the merchant typed.
    Error {
        input: CreatePaymentTryForm,
        message: String,
    },
}

/// The Acme Pay "Try it" payment-creation form (`/providers/acmepay`'s
/// `create_payment_from_provider_page` mutation target), swapped as a
/// whole (`#acmepay-payment-try`, outerHTML) so a submission's result and
/// a fresh form render together — same convention as
/// `payments::detail_fragment`.
pub fn payment_try_fragment(outcome: Option<PaymentTryOutcome>) -> Markup {
    let input = match &outcome {
        Some(PaymentTryOutcome::Error { input, .. }) => input.clone(),
        _ => CreatePaymentTryForm::default(),
    };

    html! {
        div #acmepay-payment-try {
            h3 { "Create a payment" }
            @match &outcome {
                Some(PaymentTryOutcome::Success(payment)) => {
                    p.try-it-result {
                        (components::pill(payment.tone, &payment.label))
                        " "
                        a href={ "/payments/" (payment.id) } { (payment.id.clone()) }
                        " — " (components::money(payment.amount_cents, &payment.currency))
                    }
                }
                Some(PaymentTryOutcome::Error { message, .. }) => {
                    p.try-it-error { (message) }
                }
                None => {}
            }
            form hx-post="/providers/acmepay/payments" hx-target="#acmepay-payment-try" hx-swap="outerHTML" {
                label { "Amount (minor units)" input type="number" name="amount" value=(input.amount) required; }
                label { "Currency" input type="text" name="currency" value=(input.currency) required; }
                label { "Card number" input type="text" name="card_number" value=(input.card_number) required; }
                label { "Exp month" input type="number" name="exp_month" value=(input.exp_month) required; }
                label { "Exp year" input type="number" name="exp_year" value=(input.exp_year) required; }
                label { "CVV" input type="text" name="cvv" value=(input.cvv.clone().unwrap_or_default()); }
                label { "Holder" input type="text" name="holder" value=(input.holder.clone().unwrap_or_default()); }
                label { "Reference" input type="text" name="reference" value=(input.reference.clone().unwrap_or_default()); }
                button type="submit" { "Create payment" }
            }
        }
    }
}

/// The `/providers/acmepay/webhook_endpoints` form body.
#[derive(Debug, Deserialize)]
pub struct CreateWebhookTryForm {
    pub url: String,
    pub enabled_events: Option<String>,
    pub description: Option<String>,
}

/// A webhook endpoint created through `webhook_try_fragment`'s form.
pub struct WebhookTryResult {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub secret: String,
}

/// The Acme Pay "Try it" webhook-registration form. Shows the generated
/// `whsec_...` secret once — the sandbox reveals secrets in full (spec
/// §18), and this inline result is the only place this one is ever shown.
/// Unlike the payment/shipment forms, registering an endpoint has no
/// business-validation failure mode to redraw the form for — an error
/// here is a genuine 500, so this only ever renders `None` or `Some(Ok)`.
pub fn webhook_try_fragment(result: Option<Result<WebhookTryResult, String>>) -> Markup {
    html! {
        div #acmepay-webhook-try {
            h3 { "Register a webhook endpoint" }
            @match &result {
                Some(Ok(endpoint)) => {
                    p.try-it-result {
                        a href={ "/webhooks/endpoints/" (endpoint.id) } { (endpoint.id.clone()) }
                        " → " (endpoint.url.clone())
                        " (" (endpoint.events.join(", ")) ")"
                    }
                    p { "Signing secret (shown once): " span.mono { (endpoint.secret.clone()) } }
                }
                Some(Err(message)) => {
                    p.try-it-error { (message) }
                }
                None => {}
            }
            form hx-post="/providers/acmepay/webhook_endpoints" hx-target="#acmepay-webhook-try" hx-swap="outerHTML" {
                label { "URL" input type="url" name="url" placeholder="https://example.test/hooks" required; }
                label { "Events (comma-separated, or * for all)" input type="text" name="enabled_events" value="*" placeholder="payment.captured, payment.rejected"; }
                label { "Description" input type="text" name="description"; }
                button type="submit" { "Register endpoint" }
            }
        }
    }
}

/// The `/providers/acmeship/shipments` form body — also reused to
/// repopulate the form's inputs when a submission fails validation or has
/// no coverage, instead of resetting to placeholder defaults (spec
/// `provider-try-it-forms` - "Booking with invalid input"/"Booking to an
/// uncovered destination").
#[derive(Debug, Clone, Deserialize)]
pub struct CreateShipmentTryForm {
    pub origin_postal_code: String,
    pub origin_country: String,
    pub destination_postal_code: String,
    pub destination_country: String,
    pub weight_grams: i64,
    pub length_cm: i64,
    pub width_cm: i64,
    pub height_cm: i64,
    pub declared_value_amount: i64,
    pub declared_value_currency: String,
    pub service_code: Option<String>,
    pub recipient_name: Option<String>,
    pub order_reference: Option<String>,
}

impl Default for CreateShipmentTryForm {
    fn default() -> Self {
        Self {
            origin_postal_code: "28001".to_string(),
            origin_country: "ES".to_string(),
            destination_postal_code: "08001".to_string(),
            destination_country: "ES".to_string(),
            weight_grams: 1000,
            length_cm: 20,
            width_cm: 15,
            height_cm: 10,
            declared_value_amount: 5000,
            declared_value_currency: "EUR".to_string(),
            service_code: Some("standard".to_string()),
            recipient_name: None,
            order_reference: None,
        }
    }
}

/// A shipment created through `shipment_try_fragment`'s form.
pub struct ShipmentTryResult {
    pub id: String,
    pub tracking_number: String,
    pub tone: &'static str,
    pub label: String,
    pub price_cents: i64,
    pub currency: String,
}

/// Either outcome `shipment_try_fragment` can render after a submission.
pub enum ShipmentTryOutcome {
    Success(ShipmentTryResult),
    Error {
        input: CreateShipmentTryForm,
        message: String,
    },
}

/// The Acme Ship "Try it" shipment-creation form, booking directly (no
/// separate rate-quote step) through `book_shipment_direct`.
pub fn shipment_try_fragment(outcome: Option<ShipmentTryOutcome>) -> Markup {
    let input = match &outcome {
        Some(ShipmentTryOutcome::Error { input, .. }) => input.clone(),
        _ => CreateShipmentTryForm::default(),
    };
    let is_express = input.service_code.as_deref() == Some("express_24h");

    html! {
        div #acmeship-shipment-try {
            h3 { "Create a shipment" }
            @match &outcome {
                Some(ShipmentTryOutcome::Success(shipment)) => {
                    p.try-it-result {
                        (components::pill(shipment.tone, &shipment.label))
                        " "
                        a href={ "/shipments/" (shipment.id) } { (shipment.tracking_number.clone()) }
                        " — " (components::money(shipment.price_cents, &shipment.currency))
                    }
                }
                Some(ShipmentTryOutcome::Error { message, .. }) => {
                    p.try-it-error { (message) }
                }
                None => {}
            }
            form hx-post="/providers/acmeship/shipments" hx-target="#acmeship-shipment-try" hx-swap="outerHTML" {
                label { "Origin postal code" input type="text" name="origin_postal_code" value=(input.origin_postal_code) required; }
                label { "Origin country" input type="text" name="origin_country" value=(input.origin_country) required; }
                label { "Destination postal code" input type="text" name="destination_postal_code" value=(input.destination_postal_code) required; }
                label { "Destination country" input type="text" name="destination_country" value=(input.destination_country) required; }
                label { "Weight (grams)" input type="number" name="weight_grams" value=(input.weight_grams) required; }
                label { "Length (cm)" input type="number" name="length_cm" value=(input.length_cm) required; }
                label { "Width (cm)" input type="number" name="width_cm" value=(input.width_cm) required; }
                label { "Height (cm)" input type="number" name="height_cm" value=(input.height_cm) required; }
                label { "Declared value (minor units)" input type="number" name="declared_value_amount" value=(input.declared_value_amount) required; }
                label { "Declared value currency" input type="text" name="declared_value_currency" value=(input.declared_value_currency) required; }
                label {
                    "Service"
                    select name="service_code" {
                        option value="standard" selected[!is_express] { "Standard" }
                        option value="express_24h" selected[is_express] { "Express 24h" }
                    }
                }
                label { "Recipient name" input type="text" name="recipient_name" value=(input.recipient_name.clone().unwrap_or_default()); }
                label { "Order reference" input type="text" name="order_reference" value=(input.order_reference.clone().unwrap_or_default()); }
                button type="submit" { "Create shipment" }
            }
        }
    }
}
