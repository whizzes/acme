//! Shared handles injected into every axum handler via `State<AppState>`.

use sqlx::SqlitePool;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    #[allow(dead_code)]
    pub db: SqlitePool,
    pub cfg: Config,
}
