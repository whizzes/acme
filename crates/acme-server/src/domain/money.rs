//! Money as integer minor units plus a currency code (spec §7) — never a
//! float, so a Chilean price never silently rounds.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Money {
    pub cents: i64,
    pub currency: Currency,
}

impl Money {
    pub const fn new(cents: i64, currency: Currency) -> Self {
        Self { cents, currency }
    }

    /// `None` on currency mismatch or overflow, never a silent truncation.
    pub fn checked_add(self, other: Self) -> Option<Self> {
        if self.currency != other.currency {
            return None;
        }
        self.cents.checked_add(other.cents).map(|cents| Self { cents, ..self })
    }

    /// `None` on currency mismatch or overflow, never a silent truncation.
    pub fn checked_sub(self, other: Self) -> Option<Self> {
        if self.currency != other.currency {
            return None;
        }
        self.cents.checked_sub(other.cents).map(|cents| Self { cents, ..self })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Currency {
    Clp,
    Brl,
    Eur,
    Ars,
    Mxn,
    Usd,
}

impl Currency {
    pub fn as_str(self) -> &'static str {
        match self {
            Currency::Clp => "CLP",
            Currency::Brl => "BRL",
            Currency::Eur => "EUR",
            Currency::Ars => "ARS",
            Currency::Mxn => "MXN",
            Currency::Usd => "USD",
        }
    }

    /// ISO-4217 minor-unit exponent. CLP is the deliberate zero-decimal
    /// trap from spec Appendix C — an integration that assumes "divide by
    /// 100" shows Chilean prices 100x too low.
    pub fn minor_units(self) -> u32 {
        match self {
            Currency::Clp => 0,
            _ => 2,
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown currency code `{0}`")]
pub struct UnknownCurrency(String);

impl FromStr for Currency {
    type Err = UnknownCurrency;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "CLP" => Ok(Currency::Clp),
            "BRL" => Ok(Currency::Brl),
            "EUR" => Ok(Currency::Eur),
            "ARS" => Ok(Currency::Ars),
            "MXN" => Ok(Currency::Mxn),
            "USD" => Ok(Currency::Usd),
            other => Err(UnknownCurrency(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_round_trips_through_str() {
        for c in [Currency::Clp, Currency::Brl, Currency::Eur, Currency::Ars, Currency::Mxn, Currency::Usd] {
            assert_eq!(c.as_str().parse::<Currency>().unwrap(), c);
        }
    }

    #[test]
    fn checked_add_rejects_currency_mismatch() {
        let a = Money::new(100, Currency::Usd);
        let b = Money::new(100, Currency::Eur);
        assert_eq!(a.checked_add(b), None);
    }

    #[test]
    fn checked_add_sums_same_currency() {
        let a = Money::new(100, Currency::Usd);
        let b = Money::new(50, Currency::Usd);
        assert_eq!(a.checked_add(b), Some(Money::new(150, Currency::Usd)));
    }

    #[test]
    fn clp_has_zero_minor_units() {
        assert_eq!(Currency::Clp.minor_units(), 0);
        assert_eq!(Currency::Usd.minor_units(), 2);
    }
}
