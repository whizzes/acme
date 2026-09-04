//! Library crate backing the `acme` binary (`src/main.rs`) and the
//! integration tests under `tests/`. Wiring: load config, open the DB pool,
//! run migrations, start the ticker, serve HTTP, shut down gracefully.

pub mod capture;
pub mod config;
pub mod db;
pub mod domain;
pub mod error;
pub mod sim;
pub mod state;
pub mod web;

use tokio::signal;
use tokio_util::sync::CancellationToken;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::capture::recorder::Recorder;
use crate::config::Config;
use crate::state::AppState;

pub async fn run(cfg: Config) -> anyhow::Result<()> {
    let db = db::connect(&cfg.database_url).await?;
    let clock = db::bootstrap_sim_clock(&db, &cfg).await?;
    let (recorder, _recorder_handle) = Recorder::spawn(db.clone());

    let ticker_token = CancellationToken::new();
    let ticker_handle = sim::ticker::spawn(db.clone(), clock.clone(), ticker_token.clone());

    let bind = cfg.bind;
    let state = AppState { db, cfg, clock, recorder };

    let app = web::router(state)
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new());

    tracing::info!(%bind, "acme listening");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    ticker_token.cancel();
    let _ = ticker_handle.await;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutting down");
}
