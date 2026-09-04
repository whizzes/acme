//! Runtime configuration: `ACME_`-prefixed env vars over an optional
//! `acme.toml`, over built-in defaults (spec §6).

use std::net::SocketAddr;

use chrono::{DateTime, Utc};
use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    pub public_url: String,
    pub seed: bool,
    pub seed_rng: u64,
    pub seed_scale: SeedScale,
    pub clock_multiplier: f64,
    pub clock_epoch: DateTime<Utc>,
    pub webhooks_enabled: bool,
    pub webhook_timeout_ms: u64,
    pub webhook_max_attempts: u32,
    pub request_log_retention: u32,
    pub request_log_body_limit: usize,
    pub latency_ms: u64,
    pub failure_rate: f64,
    pub log: String,
    pub dashboard_enabled: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedScale {
    Small,
    Medium,
    Large,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:2263".parse().expect("valid default bind addr"),
            database_url: "sqlite://acme.db?mode=rwc".into(),
            public_url: "http://localhost:2263".into(),
            seed: true,
            seed_rng: 42,
            seed_scale: SeedScale::Medium,
            clock_multiplier: 60.0,
            clock_epoch: Utc::now(),
            webhooks_enabled: true,
            webhook_timeout_ms: 5000,
            webhook_max_attempts: 6,
            request_log_retention: 50_000,
            request_log_body_limit: 65_536,
            latency_ms: 0,
            failure_rate: 0.0,
            log: "info,acme=debug".into(),
            dashboard_enabled: true,
        }
    }
}

impl Config {
    /// Loads defaults, then `acme.toml`, then `ACME_`-prefixed env vars.
    pub fn load() -> anyhow::Result<Self> {
        let cfg = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file("acme.toml"))
            .merge(Env::prefixed("ACME_"))
            .extract()?;
        Ok(cfg)
    }
}
