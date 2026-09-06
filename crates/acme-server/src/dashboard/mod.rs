//! Dashboard-only concerns that don't belong to any single provider or
//! resource: the live-activity broadcast hub (spec §13.4 pattern 2), fed
//! by both `capture::recorder` and the resource repos (specs/004-Dashboard.md's
//! Open Questions).

pub mod activity;
