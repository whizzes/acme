//! Thin binary wrapper — see `lib.rs` for the actual wiring.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = acme_server::config::Config::load()?;

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(&cfg.log).unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    acme_server::run(cfg).await
}
