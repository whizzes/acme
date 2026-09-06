//! Payment provider adapters. `acmepay` (M2) and `webpay` (M5 first slice,
//! specs/006-Dialects.md) are built; the other three dialects (spec
//! §10.3-§10.5) are notes in specs/007-Additional-Dialects.md.

pub mod acmepay;
pub mod webpay;
