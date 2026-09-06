//! Maps between `db::repo::shipments` rows, `sim::pricing`, and Iberex
//! Express's Spanish-language wire shapes (spec §11.2).

use base64::Engine;

use crate::domain::shipment::ShipmentStatus;
use crate::providers::shipping::iberex::dto::{BultoInput, ClienteInput};
use crate::sim::pricing::{Package, Tariff};

/// Iberex's two service levels (spec §11.2: `servicio` `1`/`2`). Each owns
/// its own `sim::pricing::Tariff`, reusing the shared ES zone matrix
/// (`sim::pricing::es_zone`) unchanged — specs/006-Dialects.md's own
/// reasoning for why this milestone adds no new zone matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IberexService {
    Seur24,
    Seur10,
}

impl IberexService {
    pub const ALL: [IberexService; 2] = [IberexService::Seur24, IberexService::Seur10];

    pub fn from_servicio(s: &str) -> Option<Self> {
        match s {
            "1" => Some(IberexService::Seur24),
            "2" => Some(IberexService::Seur10),
            _ => None,
        }
    }

    pub fn servicio_code(self) -> &'static str {
        match self {
            IberexService::Seur24 => "1",
            IberexService::Seur10 => "2",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            IberexService::Seur24 => "SEUR 24",
            IberexService::Seur10 => "SEUR 10",
        }
    }

    /// Iberex's own tariff — priced a little above Acme Ship's own
    /// `ServiceCode` tariffs so the two dialects never coincidentally
    /// quote the exact same price for the same parcel.
    pub fn tariff(self) -> Tariff {
        match self {
            IberexService::Seur24 => Tariff {
                base_cents: 470,
                included_kg: 2.0,
                per_step_cents: 160,
                increment_kg: 1.0,
                divisor: 6000.0,
                eta_min_days: 1,
                eta_max_days: 2,
            },
            IberexService::Seur10 => Tariff {
                base_cents: 990,
                included_kg: 2.0,
                per_step_cents: 240,
                increment_kg: 1.0,
                divisor: 5000.0,
                eta_min_days: 1,
                eta_max_days: 1,
            },
        }
    }
}

/// `PENDIENTE_RECOGIDA -> ... -> ENTREGADO`, plus the exception/return/
/// cancel branches (spec §11.6's Iberex column).
pub fn iberex_status(status: ShipmentStatus) -> &'static str {
    use ShipmentStatus::*;
    match status {
        Quoted | Created => "PENDIENTE_RECOGIDA",
        LabelGenerated => "ETIQUETA_GENERADA",
        PickedUp => "RECOGIDO",
        InTransit => "EN_TRANSITO",
        AtFacility => "EN_DELEGACION_DESTINO",
        OutForDelivery => "EN_REPARTO",
        DeliveryAttempted => "AUSENTE_1",
        Delivered => "ENTREGADO",
        Exception => "INCIDENCIA_DIRECCION",
        Returning => "DEVOLUCION_CURSO",
        Returned => "DEVUELTO_ORIGEN",
        Cancelled => "ANULADO",
        Lost => "EXTRAVIADO",
    }
}

pub fn to_pricing_package(b: &BultoInput) -> Package {
    Package {
        weight_grams: (b.peso * 1000.0).round() as i64,
        length_cm: b.largo,
        width_cm: b.ancho,
        height_cm: b.alto,
    }
}

/// Same reasoning as `acmeship::map::address_json`: opaque JSON in
/// `shipments.origin`/`destination` (spec §7.4's columns are just `TEXT`).
pub fn cliente_json(c: &ClienteInput) -> String {
    serde_json::json!({
        "nombre": c.nombre,
        "direccion": c.direccion,
        "codigoPostal": c.codigo_postal,
        "poblacion": c.poblacion,
        "pais": c.pais,
        "telefono": c.telefono,
        "email": c.email,
    })
    .to_string()
}

pub fn fecha_dd_mm_yyyy_hh_mm(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%d/%m/%Y %H:%M").to_string()
}

/// Placeholder ZPL for a parcel label — enough to look plausible, not a
/// real print-ready template (spec §11.2's own example is likewise
/// truncated: `"^XA^FO50,50^ADN,36,20^FD…^XZ"`).
pub fn zpl_label(codigo_bulto: &str) -> String {
    format!("^XA^FO50,50^ADN,36,20^FD{codigo_bulto}^FS^XZ")
}

pub fn pdf_label_base64(codigo_bulto: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("PDF-LABEL:{codigo_bulto}"))
}
