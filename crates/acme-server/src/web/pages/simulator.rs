//! `/simulator` (spec §13.6, specs/008-Simulation.md item 6): clock
//! controls, global latency/failure sliders, the fault-rule table, and a
//! scenario cheat-sheet. Seed controls are trimmed to a real **reset**
//! only — see the spec's own Caveats for why scale presets stay
//! display-only until M7's Faker world exists to back them.

use axum::extract::State;
use maud::{Markup, html};

use crate::db::repo::{faults, sim as sim_settings};
use crate::domain::payment::PaymentStatus;
use crate::domain::scenario::{PaymentScenario, ShipmentScenario};
use crate::domain::shipment::ShipmentStatus;
use crate::error::AppError;
use crate::state::AppState;
use crate::web::layout::{Ctx, NavItem, layout};
use crate::web::pages::components;

pub async fn page(State(state): State<AppState>) -> Result<Markup, AppError> {
    let clock_label = components::timestamp(state.clock.now());
    let fragment = render_fragment(&state).await?;

    Ok(layout(
        &Ctx {
            title: "Simulator",
            clock_label: &clock_label,
            active: NavItem::Simulator,
        },
        fragment,
    ))
}

/// The whole page body, re-rendered wholesale after any mutation — same
/// "swap the whole detail" pattern `web::pages::payments::detail_fragment`
/// already established (no single row of its own to swap here either).
pub async fn render_fragment(state: &AppState) -> Result<Markup, AppError> {
    let settings = sim_settings::get(&state.db).await?;
    let fault_rows = faults::list(&state.db).await?;

    Ok(html! {
        div #simulator {
            h1 { "Simulator" }

            h2 { "Clock" }
            dl.kv {
                dt { "sim time" } dd.mono { (components::timestamp(state.clock.now())) }
                dt { "speed" } dd.mono {
                    @if settings.paused { "paused (resumes at " (settings.multiplier.to_string()) "×)" }
                    @else { (state.clock.multiplier().to_string()) "×" }
                }
            }
            div.action-bar {
                form hx-post="/sim/clock" hx-target="#simulator" hx-swap="outerHTML" {
                    input type="hidden" name="action" value="set_speed";
                    input type="number" step="any" name="multiplier" placeholder="multiplier, e.g. 60" value=(settings.multiplier.to_string());
                    button type="submit" { "Set speed" }
                }
                form hx-post="/sim/clock" hx-target="#simulator" hx-swap="outerHTML" {
                    input type="hidden" name="action" value=(if settings.paused { "resume" } else { "pause" });
                    button type="submit" { @if settings.paused { "Resume" } @else { "Pause" } }
                }
                form hx-post="/sim/clock" hx-target="#simulator" hx-swap="outerHTML" {
                    input type="hidden" name="action" value="jump";
                    input type="number" name="jump_seconds" placeholder="seconds, e.g. 21600 for 6h";
                    button type="submit" { "Jump" }
                }
            }

            h2 { "Global latency / failure" }
            p.hint { "Applied to every provider request, independent of any fault rule below." }
            form.action-bar hx-post="/sim/settings" hx-target="#simulator" hx-swap="outerHTML" {
                label { "latency (ms)"
                    input type="number" name="latency_ms" value=(settings.latency_ms.to_string());
                }
                label { "failure rate (0.0–1.0)"
                    input type="number" step="0.01" min="0" max="1" name="failure_rate" value=(settings.failure_rate.to_string());
                }
                button type="submit" { "Apply" }
            }

            h2 { "Fault rules" }
            table.data {
                thead { tr {
                    th { "provider" } th { "method" } th { "path" } th { "mode" } th.num { "status" } th.num { "prob." } th.num { "remaining" } th { "" }
                } }
                tbody {
                    @for fault in &fault_rows { (fault_row(fault)) }
                }
            }
            @if fault_rows.is_empty() {
                (components::empty_state("No fault rules configured — every request runs clean.", None))
            }

            h3 { "New fault rule" }
            form.action-bar hx-post="/sim/faults" hx-target="#simulator" hx-swap="outerHTML" {
                input type="text" name="provider_slug" placeholder="provider (blank = all)";
                input type="text" name="method" placeholder="method (blank = all)";
                input type="text" name="path_glob" placeholder="path glob, e.g. /acmepay/v1/*" required;
                select name="mode" {
                    option value="error" { "error" }
                    option value="latency" { "latency" }
                    option value="timeout" { "timeout" }
                    option value="malformed" { "malformed" }
                    option value="rate_limit" { "rate_limit" }
                }
                input type="number" name="http_status" placeholder="http status (error mode)";
                input type="text" name="error_code" placeholder="error code (error mode)";
                input type="number" name="latency_ms" placeholder="latency ms (latency mode)";
                input type="number" step="0.01" min="0" max="1" name="probability" placeholder="probability (default 1.0)";
                input type="number" name="remaining" placeholder="fire N times (blank = forever)";
                input type="text" name="note" placeholder="note, e.g. ticket ACME-411";
                button type="submit" { "Create rule" }
            }

            h2 { "Scenario cheat-sheet" }
            p.hint { "Payment magic values (spec §9.1)." }
            table.data {
                thead { tr { th { "trigger" } th { "scenario" } } }
                tbody {
                    tr { td.mono { "PAN 4111 1111 1111 1111 / 5555 5555 5555 4444" } td { (PaymentScenario::Approve.as_str()) } }
                    tr { td.mono { "PAN 4000 0000 0000 0002" } td { (PaymentScenario::DeclineInsufficientFunds.as_str()) } }
                    tr { td.mono { "PAN 4000 0000 0000 0069" } td { (PaymentScenario::DeclineExpiredCard.as_str()) } }
                    tr { td.mono { "PAN 4000 0000 0000 0127" } td { (PaymentScenario::DeclineInvalidCvv.as_str()) } }
                    tr { td.mono { "PAN 4000 0000 0000 0119" } td { (PaymentScenario::ProcessingError.as_str()) } }
                    tr { td.mono { "PAN 4000 0000 0000 3220" } td { (PaymentScenario::ThreeDsChallenge.as_str()) } }
                    tr { td.mono { "PAN 4000 0000 0000 3063" } td { (PaymentScenario::ThreeDsFail.as_str()) } }
                    tr { td.mono { "PAN 4000 0000 0000 0341" } td { (PaymentScenario::Chargeback.as_str()) } }
                    tr { td.mono { "PAN 4000 0000 0000 9995" } td { (PaymentScenario::SlowApproval.as_str()) } }
                    tr { td.mono { "PAN 4242 4242 4242 4241" } td { (PaymentScenario::InvalidNumber.as_str()) } }
                    tr { td.mono { "amount ends 13 / email decline@acme.test" } td { (PaymentScenario::DeclineDoNotHonor.as_str()) } }
                    tr { td.mono { "amount ends 51" } td { (PaymentScenario::ManualReview.as_str()) } }
                    tr { td.mono { "amount ends 99" } td { (PaymentScenario::ProviderError.as_str()) } }
                    tr { td.mono { "amount > 5,000,000 minor units" } td { (PaymentScenario::RiskReject.as_str()) } }
                    tr { td.mono { "PIX/boleto amount ends 77" } td { (PaymentScenario::NeverPaid.as_str()) } }
                }
            }
            p.hint { "Shipping magic values (spec §9.2)." }
            table.data {
                thead { tr { th { "trigger" } th { "scenario" } } }
                tbody {
                    tr { td.mono { "postal code 00000" } td { (ShipmentScenario::NoCoverage.as_str()) } }
                    tr { td.mono { "postal code starts 999" } td { (ShipmentScenario::AddressException.as_str()) } }
                    tr { td.mono { "postal code 07001 (ES) / 7550000 (CL)" } td { (ShipmentScenario::RemoteArea.as_str()) } }
                    tr { td.mono { "package > 30,000g or any dimension > 150cm" } td { (ShipmentScenario::Oversized.as_str()) } }
                    tr { td.mono { "declared value > 300,000 minor units" } td { (ShipmentScenario::RequiresInsurance.as_str()) } }
                    tr { td.mono { "recipient name contains NADIE / NOBODY" } td { (ShipmentScenario::FailedAttempts.as_str()) } }
                    tr { td.mono { "recipient name contains PERDIDO / LOST" } td { (ShipmentScenario::Lost.as_str()) } }
                    tr { td.mono { "order reference contains SLOWSHIP" } td { (ShipmentScenario::Delayed.as_str()) } }
                    tr { td.mono { "order reference contains FASTSHIP" } td { (ShipmentScenario::Express.as_str()) } }
                }
            }
            p.hint {
                "Every status name above (" (components::enum_str(PaymentStatus::Captured)) ", "
                (components::enum_str(ShipmentStatus::Delivered)) ", …) matches "
                code { "domain::payment::PaymentStatus" } "/" code { "domain::shipment::ShipmentStatus" }
                " exactly — the dashboard's action bars offer nothing this table can't explain."
            }

            h2 { "Seed" }
            p.hint {
                "Scale presets (small/medium/large) aren't wired to a generator yet — M7's Faker world. "
                "Reset below clears the dynamic tables and reseeds the one demo merchant/credential fixture."
            }
            form hx-post="/sim/reset" hx-target="#simulator" hx-swap="outerHTML"
                onsubmit="return confirm('This deletes every merchant, credential, payment, shipment, webhook, fault rule, and traffic exchange, then reseeds the one demo merchant. Continue?')" {
                button type="submit" { "Reset and reseed" }
            }
        }
    })
}

fn fault_row(fault: &faults::FaultRow) -> Markup {
    let (tone, label) = if fault.active {
        ("ok", "active")
    } else {
        ("idle", "spent")
    };
    html! {
        tr {
            td { (fault.provider_slug.clone().unwrap_or_else(|| "*".to_string())) }
            td { (fault.method.clone().unwrap_or_else(|| "*".to_string())) }
            td.mono { (fault.path_glob.clone()) }
            td { (fault.mode.clone()) }
            td.num { @if let Some(status) = fault.http_status { (status) } @else { "—" } }
            td.num { (fault.probability.to_string()) }
            td.num { @if let Some(remaining) = fault.remaining { (remaining) } @else { "∞" } }
            td {
                (components::pill(tone, label))
                form hx-delete={ "/sim/faults/" (fault.id) } hx-target="#simulator" hx-swap="outerHTML" {
                    button type="submit" { "Delete" }
                }
            }
        }
    }
}
