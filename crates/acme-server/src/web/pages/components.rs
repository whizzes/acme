//! Shared render helpers (spec §13.5: "exactly one definition of a
//! payment row"). Every list/detail page builds its markup from these
//! rather than hand-rolling its own status pill or money string.

use chrono::{DateTime, Utc};
use maud::{Markup, PreEscaped, html};
use serde_json::Value;

use crate::domain::money::Currency;
use crate::domain::payment::PaymentStatus;
use crate::domain::shipment::ShipmentStatus;

/// `(tone, label)` — `tone` is one of the six state tokens from spec
/// §13.2 (`ok`/`warn`/`bad`/`idle`), used as a CSS class only, never text.
pub fn payment_tone(status: PaymentStatus) -> (&'static str, &'static str) {
    use PaymentStatus::*;
    let label = status_label_payment(status);
    let tone = match status {
        Captured | Authorized | Refunded => "ok",
        Pending | Created | Disputed | PartiallyRefunded => "warn",
        Rejected | ChargedBack => "bad",
        Cancelled | Expired => "idle",
    };
    (tone, label)
}

fn status_label_payment(status: PaymentStatus) -> &'static str {
    use PaymentStatus::*;
    match status {
        Created => "Created",
        Pending => "Pending",
        Authorized => "Authorized",
        Captured => "Captured",
        PartiallyRefunded => "Partially refunded",
        Refunded => "Refunded",
        Rejected => "Rejected",
        Cancelled => "Cancelled",
        Expired => "Expired",
        Disputed => "Disputed",
        ChargedBack => "Charged back",
    }
}

pub fn shipment_tone(status: ShipmentStatus) -> (&'static str, &'static str) {
    use ShipmentStatus::*;
    let label = status_label_shipment(status);
    let tone = match status {
        Delivered => "ok",
        Exception | Lost => "bad",
        Cancelled | Returned => "idle",
        _ => "warn",
    };
    (tone, label)
}

fn status_label_shipment(status: ShipmentStatus) -> &'static str {
    use ShipmentStatus::*;
    match status {
        Quoted => "Quoted",
        Created => "Created",
        LabelGenerated => "Label generated",
        PickedUp => "Picked up",
        InTransit => "In transit",
        AtFacility => "At facility",
        OutForDelivery => "Out for delivery",
        DeliveryAttempted => "Delivery attempted",
        Delivered => "Delivered",
        Exception => "Exception",
        Returning => "Returning",
        Returned => "Returned",
        Cancelled => "Cancelled",
        Lost => "Lost",
    }
}

/// The serde `snake_case` wire form of an enum status, e.g.
/// `PaymentStatus::PartiallyRefunded` -> `"partially_refunded"` — the
/// value dashboard forms/buttons send back on `{to_status}`.
pub fn enum_str<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub fn pill(tone: &str, label: &str) -> Markup {
    html! { span class={ "pill pill--" (tone) } { (label) } }
}

/// Minor-unit-aware money formatting (spec Appendix C's CLP zero-decimal
/// trap): `4599000`/`CLP` renders as `4599000 CLP`, `459900`/`USD` as
/// `4599.00 USD`.
pub fn money(cents: i64, currency: &str) -> String {
    let minor = currency
        .parse::<Currency>()
        .map(Currency::minor_units)
        .unwrap_or(2);
    if minor == 0 {
        return format!("{cents} {currency}");
    }
    let divisor = 10i64.pow(minor);
    let whole = cents / divisor;
    let frac = (cents % divisor).abs();
    format!("{whole}.{frac:0width$} {currency}", width = minor as usize)
}

/// Mono, sortable, sim time (spec §13.2's type split: anything a machine
/// produced is set in mono).
pub fn timestamp(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn empty_state(message: &str, curl: Option<&str>) -> Markup {
    html! {
        div.empty-state {
            p { (message) }
            @if let Some(curl) = curl {
                pre.curl { (curl) }
            }
        }
    }
}

/// A tiny server-side JSON tokenizer (spec §13.6: "a tiny JSON tokenizer
/// emitting spans — no client library"), used by the traffic detail's
/// *pretty* body view and the payment/shipment detail's dialect-vs-
/// normalized comparison.
pub fn json_pretty(raw: &str) -> Markup {
    match serde_json::from_str::<Value>(raw) {
        Ok(value) => html! { pre.json-body { (json_value(&value, 0)) } },
        Err(_) => html! { pre.json-body { (raw) } },
    }
}

fn indent(depth: usize) -> PreEscaped<String> {
    PreEscaped("  ".repeat(depth))
}

fn json_value(value: &Value, depth: usize) -> Markup {
    match value {
        Value::Null => html! { span.json-null { "null" } },
        Value::Bool(b) => html! { span.json-bool { (b.to_string()) } },
        Value::Number(n) => html! { span.json-num { (n.to_string()) } },
        Value::String(s) => html! { span.json-str { "\"" (s) "\"" } },
        Value::Array(items) => html! {
            "["
            @if !items.is_empty() { br; }
            @for (i, item) in items.iter().enumerate() {
                (indent(depth + 1))
                (json_value(item, depth + 1))
                @if i + 1 < items.len() { "," }
                br;
            }
            @if !items.is_empty() { (indent(depth)) }
            "]"
        },
        Value::Object(map) => html! {
            "{"
            @if !map.is_empty() { br; }
            @for (i, (k, v)) in map.iter().enumerate() {
                (indent(depth + 1))
                span.json-key { "\"" (k) "\"" }
                ": "
                (json_value(v, depth + 1))
                @if i + 1 < map.len() { "," }
                br;
            }
            @if !map.is_empty() { (indent(depth)) }
            "}"
        },
    }
}

/// A minimal inline sparkline (spec §13.2's "captured 71% ▁▂▅▇▆▃▂▁") — one
/// bar per bucket, tallest bucket at full height, zero-filled gaps.
pub fn sparkline(counts: &[i64]) -> Markup {
    let max = counts.iter().copied().max().unwrap_or(0).max(1);
    let width = 6 * counts.len().max(1) as i32;
    html! {
        svg.sparkline viewBox={ "0 0 " (width) " 20" } preserveAspectRatio="none" {
            @for (i, &count) in counts.iter().enumerate() {
                @let height = ((count as f64 / max as f64) * 18.0).max(1.0) as i32;
                @let x = i as i32 * 6;
                rect x=(x) y=(20 - height) width="4" height=(height) {}
            }
        }
    }
}
