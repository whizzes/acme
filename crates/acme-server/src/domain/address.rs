//! Postal address, mirroring the `addresses` table (spec §7.2).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Address {
    pub contact_name: Option<String>,
    pub phone: Option<String>,
    pub line1: String,
    pub line2: Option<String>,
    pub district: Option<String>,
    pub city: String,
    pub region: Option<String>,
    pub postal_code: String,
    pub country: String,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub residential: bool,
}
