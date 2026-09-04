//! The inbound half of the traffic inspector (spec §21.1-§21.6), pulled
//! into M1 by §21.14. Outbound (webhook) capture lands in M4.

pub mod layer;
pub mod recorder;
pub mod redact;
pub mod trace;
