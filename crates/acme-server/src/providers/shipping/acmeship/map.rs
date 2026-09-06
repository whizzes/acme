//! Maps between `db::repo::shipments` rows, `sim::pricing`, and Acme Ship's
//! wire DTOs.

use crate::db::repo::shipments::ShipmentRow;
use crate::providers::shipping::acmeship::dto::{
    AddressInput, MoneyOutput, PackageInput, Shipment,
};
use crate::sim::pricing::Package;

pub fn to_pricing_package(p: &PackageInput) -> Package {
    Package {
        weight_grams: p.weight_grams,
        length_cm: p.length_cm,
        width_cm: p.width_cm,
        height_cm: p.height_cm,
    }
}

/// Address inputs are stored as opaque JSON text in `shipments.origin`/
/// `destination` (spec §7.4's columns are just `TEXT`) — this crate never
/// needs to query into them, only round-trip and display them.
pub fn address_json(a: &AddressInput) -> String {
    serde_json::json!({
        "postal_code": a.postal_code,
        "city": a.city,
        "country": a.country,
        "residential": a.residential.unwrap_or(false),
    })
    .to_string()
}

pub fn shipment_to_dto(row: &ShipmentRow) -> Shipment {
    Shipment {
        id: row.id.to_string(),
        object: "shipment".to_string(),
        status: row.status,
        tracking_number: row.tracking_number.clone(),
        carrier: row.carrier_code.clone(),
        service_code: row.service_code.clone(),
        price: MoneyOutput {
            value: row.price_cents,
            currency: row.currency.clone(),
        },
        eta: row.eta_at.map(|t| t.to_rfc3339()),
        label_url: row.label_url.clone(),
        return_tracking_number: row.return_tracking_number.clone(),
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    }
}
