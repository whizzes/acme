//! Cross-cutting HTTP concerns shared by every provider adapter (spec §5's
//! canonical layout groups these under `http/`): per-dialect auth,
//! idempotency, and OpenAPI assembly.

pub mod admin;
pub mod auth;
pub mod idempotency;
pub mod openapi;
