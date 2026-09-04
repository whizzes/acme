//! Shared handles injected into every axum handler via `State<AppState>`.

use sqlx::SqlitePool;

use crate::capture::recorder::Recorder;
use crate::config::Config;
use crate::sim::clock::SimClock;

#[derive(Clone)]
pub struct AppState {
    #[allow(dead_code)]
    pub db: SqlitePool,
    pub cfg: Config,
    pub clock: SimClock,
    /// Unmounted until M2 adds provider routers to wrap in
    /// `capture::layer::record_exchange` — see `.agents/docs/domain.md`.
    #[allow(dead_code)]
    pub recorder: Recorder,
}
