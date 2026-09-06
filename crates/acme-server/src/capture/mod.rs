//! The traffic inspector (spec §21.1-§21.6), pulled into M1 by §21.14.
//! Inbound capture is `layer`/`recorder`; outbound (webhook) capture is
//! `dashboard::dispatcher` writing through the same `recorder`, added M4.

pub mod layer;
pub mod recorder;
pub mod redact;
pub mod trace;
