//! Binary entry point: load config, open the DB pool, wire the HTTP server,
//! shut down gracefully.

mod config;
mod db;
mod error;
mod state;
mod web;

use tokio::signal;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::load()?;

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(&cfg.log).unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let db = db::connect(&cfg.database_url).await?;
    let bind = cfg.bind;
    let state = AppState { db, cfg };

    let app = web::router(state)
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new());

    tracing::info!(%bind, "acme listening");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

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
