//! Library crate backing the `acme` binary (`src/main.rs`) and the
//! integration tests under `tests/`. Wiring: load config, open the DB pool,
//! run migrations, start the ticker, serve HTTP, shut down gracefully.

pub mod capture;
pub mod config;
pub mod dashboard;
pub mod db;
pub mod domain;
pub mod error;
pub mod http;
pub mod providers;
pub mod sim;
pub mod state;
pub mod web;

use tokio::signal;
use tokio_util::sync::CancellationToken;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::capture::recorder::Recorder;
use crate::config::Config;
use crate::dashboard::activity;
use crate::state::AppState;

pub async fn run(cfg: Config) -> anyhow::Result<()> {
    let db = db::connect(&cfg.database_url).await?;
    let clock = db::bootstrap_sim_clock(&db, &cfg).await?;
    if cfg.seed {
        db::bootstrap_demo_credentials(&db).await?;
    }
    let (recorder, _recorder_handle) = Recorder::spawn(db.clone());

    let activity_hub = activity::Hub::new();
    activity::init(activity_hub.clone());

    let ticker_token = CancellationToken::new();
    let ticker_handle = sim::ticker::spawn(db.clone(), clock.clone(), ticker_token.clone());

    let http_client = reqwest::Client::new();

    let bind = cfg.bind;
    let webhooks_enabled = cfg.webhooks_enabled;
    let state = AppState {
        db,
        cfg,
        clock,
        recorder,
        activity: activity_hub,
        http_client,
    };

    let dispatcher_token = CancellationToken::new();
    if webhooks_enabled {
        tracing::info!("webhook dispatcher: enabled, starting poll loop");
    } else {
        tracing::warn!(
            "webhook dispatcher: disabled via config (webhooks_enabled=false) — deliveries will queue as 'pending' but never be attempted"
        );
    }
    let dispatcher_handle = webhooks_enabled
        .then(|| dashboard::dispatcher::spawn(state.clone(), dispatcher_token.clone()));

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
    dispatcher_token.cancel();
    if let Some(handle) = dispatcher_handle {
        let _ = handle.await;
    }

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
