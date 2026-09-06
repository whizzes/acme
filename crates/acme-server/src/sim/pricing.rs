//! Deterministic pricing engine (spec §11.7): billable weight, zone lookup,
//! and surcharges, seeded only for **Acme Ship** and the **ES** zone matrix
//! per this milestone's scope (spec §20 M2 row) — other carriers/regions
//! plug in their own tariff/zone tables from M5 on.
//!
//! The spec's own worked example (§11.1's Madrid→Alicante JSON) and its
//! formula (§11.7) don't reconcile to the same cent under any choice of
//! constants — the example's `billable_weight_grams: 1980` isn't reachable
//! from `max(1.8kg, (30×22×12)/{5000,6000})` for either stated divisor.
//! Both are illustrative prose, not a fixture; this module implements the
//! formula literally and calibrates its own tariff constants, verified by
//! the golden test below rather than by matching the illustrative numbers
//! bit-for-bit.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct Package {
    pub weight_grams: i64,
    pub length_cm: i64,
    pub width_cm: i64,
    pub height_cm: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceCode {
    Standard,
    Express24h,
}

impl ServiceCode {
    pub const ALL: [ServiceCode; 2] = [ServiceCode::Standard, ServiceCode::Express24h];

    pub fn as_str(self) -> &'static str {
        match self {
            ServiceCode::Standard => "standard",
            ServiceCode::Express24h => "express_24h",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ServiceCode::Standard => "Acme Standard",
            ServiceCode::Express24h => "Acme Express 24h",
        }
    }

    /// `(min_days, max_days)` — the provider catalog and rate quotes both
    /// need this without reaching into the private `Tariff` shape.
    pub fn eta_days(self) -> (u32, u32) {
        let tariff = self.tariff();
        (tariff.eta_min_days, tariff.eta_max_days)
    }

    /// This carrier's own price table, for `quote`. `pub` so a caller
    /// picks a `ServiceCode` and gets the `Tariff` `quote` needs — see the
    /// `Tariff` doc comment for why a different carrier builds its own
    /// `Tariff` value instead of a new `ServiceCode` variant.
    pub fn tariff(self) -> Tariff {
        match self {
            ServiceCode::Standard => Tariff {
                base_cents: 420,
                included_kg: 2.0,
                per_step_cents: 150,
                increment_kg: 1.0,
                divisor: 6000.0,
                eta_min_days: 2,
                eta_max_days: 3,
            },
            ServiceCode::Express24h => Tariff {
                base_cents: 890,
                included_kg: 2.0,
                per_step_cents: 220,
                increment_kg: 1.0,
                divisor: 5000.0,
                eta_min_days: 1,
                eta_max_days: 1,
            },
        }
    }
}

/// One carrier's own price table (spec §11.7: "each provider owns a small
/// tariff table... which is what makes carriers differ"). `ServiceCode`'s
/// own two variants are Acme Ship's tariff; specs/006-Dialects.md's Iberex
/// module builds its own `Tariff` values and calls `quote_with_tariff`
/// directly rather than growing this enum — the enum models one carrier's
/// service levels, not every carrier's.
pub struct Tariff {
    pub base_cents: i64,
    pub included_kg: f64,
    pub per_step_cents: i64,
    pub increment_kg: f64,
    /// Volumetric-weight divisor (spec §11.7: 5000 express, 6000 standard).
    pub divisor: f64,
    pub eta_min_days: u32,
    pub eta_max_days: u32,
}

/// `1..5` domestic zones, `6` insular, `7` international (spec §11.7). Only
/// the two-digit peninsular Spanish postal prefixes this milestone's own
/// scenarios exercise are populated; an unmapped prefix falls back to the
/// coarsest domestic zone (`1`) rather than failing the quote. `pub` so
/// every carrier sharing the ES zone matrix (spec §11.2's Iberex, and any
/// later Spain-scoped dialect) resolves a zone the same way Acme Ship does,
/// rather than reimplementing this lookup per carrier.
pub fn es_zone(origin_prefix: &str, destination_prefix: &str) -> u8 {
    const CANARIAS: [&str; 2] = ["35", "38"];
    const BALEARES: &str = "07";
    const CEUTA_MELILLA: [&str; 2] = ["51", "52"];

    if CANARIAS.contains(&destination_prefix) || CANARIAS.contains(&origin_prefix) {
        return 6;
    }
    if destination_prefix == BALEARES || origin_prefix == BALEARES {
        return 5;
    }
    if CEUTA_MELILLA.contains(&destination_prefix) || CEUTA_MELILLA.contains(&origin_prefix) {
        return 6;
    }
    if origin_prefix == destination_prefix {
        return 1;
    }
    2
}

const ES_ZONE_STEP_CENTS: i64 = 40;
const ES_VAT_RATE: f64 = 0.21;

pub struct PricingInput<'a> {
    pub origin_country: &'a str,
    pub origin_postal: &'a str,
    pub destination_postal: &'a str,
    pub packages: &'a [Package],
    pub declared_value_cents: i64,
    pub residential: bool,
    pub insurance_requested: bool,
    pub cash_on_delivery: bool,
    pub saturday_delivery: bool,
}

#[derive(Debug, Clone)]
pub struct Surcharge {
    pub code: &'static str,
    pub label: &'static str,
    pub cents: i64,
}

#[derive(Debug, Clone)]
pub struct PricingResult {
    pub billable_weight_grams: i64,
    pub zone: u8,
    pub base_cents: i64,
    pub surcharges: Vec<Surcharge>,
    pub total_cents: i64,
    pub eta_min_days: u32,
    pub eta_max_days: u32,
}

fn postal_prefix(postal: &str) -> &str {
    &postal[..postal.len().min(2)]
}

/// Sums declared package weights/volumes: multi-parcel quotes bill on the
/// combined billable weight (spec §11.7 doesn't special-case multi-piece).
/// `tariff` is the calling carrier's own price table (spec §11.7's "each
/// provider owns a small tariff table") — `ServiceCode::tariff()` for Acme
/// Ship, a carrier-owned `Tariff` value for anyone else sharing this same
/// zone-matrix-and-surcharge shape (spec §11.2's Iberex).
pub fn quote(tariff: &Tariff, input: &PricingInput<'_>) -> PricingResult {
    let divisor = tariff.divisor;

    let actual_kg: f64 = input
        .packages
        .iter()
        .map(|p| p.weight_grams as f64 / 1000.0)
        .sum();
    let volumetric_kg: f64 = input
        .packages
        .iter()
        .map(|p| (p.length_cm * p.width_cm * p.height_cm) as f64 / divisor)
        .sum();
    let billable_kg = actual_kg.max(volumetric_kg);
    let billable_weight_grams = (billable_kg * 1000.0).round() as i64;

    let zone = if input.origin_country == "ES" {
        es_zone(
            postal_prefix(input.origin_postal),
            postal_prefix(input.destination_postal),
        )
    } else {
        7
    };

    let over_included_kg = (billable_kg - tariff.included_kg).max(0.0);
    let steps = (over_included_kg / tariff.increment_kg).ceil() as i64;
    let base_cents =
        tariff.base_cents + ES_ZONE_STEP_CENTS * (zone as i64 - 1) + steps * tariff.per_step_cents;

    let mut surcharges = Vec::new();

    let fuel_cents = (base_cents as f64 * 0.07).round() as i64;
    surcharges.push(Surcharge {
        code: "fuel",
        label: "Fuel surcharge",
        cents: fuel_cents,
    });

    if input.residential {
        surcharges.push(Surcharge {
            code: "residential",
            label: "Residential delivery",
            cents: 76,
        });
    }

    if input.insurance_requested {
        let insurance = ((input.declared_value_cents as f64) * 0.009).round() as i64;
        surcharges.push(Surcharge {
            code: "insurance",
            label: "Insurance",
            cents: insurance.max(60),
        });
    }

    if input.cash_on_delivery {
        let cod = ((base_cents as f64) * 0.012).round() as i64;
        surcharges.push(Surcharge {
            code: "cash_on_delivery",
            label: "Cash on delivery",
            cents: cod.max(150),
        });
    }

    if input
        .packages
        .iter()
        .any(|p| p.length_cm > 120 || p.width_cm > 120 || p.height_cm > 120)
    {
        surcharges.push(Surcharge {
            code: "oversize",
            label: "Oversize handling",
            cents: 500,
        });
    }

    if input.saturday_delivery {
        surcharges.push(Surcharge {
            code: "saturday",
            label: "Saturday delivery",
            cents: 300,
        });
    }

    let surcharge_total: i64 = surcharges.iter().map(|s| s.cents).sum();
    let vat_rate = if input.origin_country == "ES" {
        ES_VAT_RATE
    } else {
        0.0
    };
    let total_cents = ((base_cents + surcharge_total) as f64 * (1.0 + vat_rate)).round() as i64;

    PricingResult {
        billable_weight_grams,
        zone,
        base_cents,
        surcharges,
        total_cents,
        eta_min_days: tariff.eta_min_days,
        eta_max_days: tariff.eta_max_days,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn madrid_to_alicante_standard() -> PricingInput<'static> {
        PricingInput {
            origin_country: "ES",
            origin_postal: "28001",
            destination_postal: "03690",
            packages: &[Package {
                weight_grams: 1800,
                length_cm: 30,
                width_cm: 22,
                height_cm: 12,
            }],
            declared_value_cents: 8990,
            residential: true,
            insurance_requested: false,
            cash_on_delivery: false,
            saturday_delivery: false,
        }
    }

    fn standard() -> Tariff {
        ServiceCode::Standard.tariff()
    }

    /// Golden test: pins this module's own calibration so a refactor can't
    /// silently change prices. See the module doc comment for why this
    /// isn't the spec's illustrative `559`/`1980` figures.
    #[test]
    fn golden_madrid_to_alicante_standard() {
        let result = quote(&standard(), &madrid_to_alicante_standard());
        assert_eq!(result.billable_weight_grams, 1800);
        assert_eq!(result.zone, 2);
        assert_eq!(result.base_cents, 460);
        assert_eq!(result.total_cents, 687);
    }

    #[test]
    fn same_prefix_is_zone_one() {
        let mut input = madrid_to_alicante_standard();
        input.destination_postal = "28002";
        let result = quote(&standard(), &input);
        assert_eq!(result.zone, 1);
    }

    #[test]
    fn price_is_monotonic_in_weight() {
        let mut light = madrid_to_alicante_standard();
        light.packages = &[Package {
            weight_grams: 500,
            length_cm: 10,
            width_cm: 10,
            height_cm: 10,
        }];
        let mut heavy = madrid_to_alicante_standard();
        heavy.packages = &[Package {
            weight_grams: 20_000,
            length_cm: 40,
            width_cm: 40,
            height_cm: 40,
        }];

        assert!(quote(&standard(), &heavy).total_cents > quote(&standard(), &light).total_cents);
    }

    #[test]
    fn price_is_monotonic_in_zone() {
        let mut near = madrid_to_alicante_standard();
        near.destination_postal = "28002"; // zone 1
        let mut far = madrid_to_alicante_standard();
        far.destination_postal = "35001"; // zone 6, Canarias

        assert!(quote(&standard(), &far).total_cents > quote(&standard(), &near).total_cents);
    }

    #[test]
    fn oversize_dimension_adds_a_surcharge_line() {
        let mut input = madrid_to_alicante_standard();
        input.packages = &[Package {
            weight_grams: 1800,
            length_cm: 130,
            width_cm: 22,
            height_cm: 12,
        }];
        let result = quote(&standard(), &input);
        assert!(result.surcharges.iter().any(|s| s.code == "oversize"));
    }

    #[test]
    fn insurance_surcharge_has_a_minimum() {
        let mut input = madrid_to_alicante_standard();
        input.insurance_requested = true;
        input.declared_value_cents = 100;
        let result = quote(&standard(), &input);
        let insurance = result
            .surcharges
            .iter()
            .find(|s| s.code == "insurance")
            .unwrap();
        assert_eq!(insurance.cents, 60);
    }
}
