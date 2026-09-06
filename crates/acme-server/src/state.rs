//! Shared handles injected into every axum handler via `State<AppState>`.

use sqlx::SqlitePool;

use crate::capture::recorder::Recorder;
use crate::config::Config;
use crate::dashboard::activity;
use crate::sim::clock::SimClock;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub cfg: Config,
    pub clock: SimClock,
    pub recorder: Recorder,
    pub activity: activity::Hub,
}
