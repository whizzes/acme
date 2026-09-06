//! Provider adapters (spec §5): dialect in, dialect out, no business logic.
//! Nothing in here runs `sqlx::query*` directly — only `db::repo` does.

pub mod payments;
pub mod shipping;
