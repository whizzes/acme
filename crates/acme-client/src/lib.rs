//! Generated client for ACME's own API surface — Acme Pay (`/acmepay/v1`)
//! and Acme Ship (`/acmeship/v1`) — see `specs/1-Acme-Client.md`.
//!
//! `progenitor` generates one `Client` per spec document, so each service
//! gets its own private module here (`pay`, `ship`), both regenerated
//! from `assets/openapi.json`/`assets/openapi-ship.json` by
//! `cargo xtask export-openapi` — never hand edited.

mod pay {
    progenitor::generate_api!(spec = "../../assets/openapi.json");
}

mod ship {
    progenitor::generate_api!(spec = "../../assets/openapi-ship.json");
}

pub use pay::Client as PayClient;
pub use pay::types as pay_types;
pub use ship::Client as ShipClient;
pub use ship::types as ship_types;

/// Both ACME services behind one base URL — `pay` at `/acmepay/v1`,
/// `ship` at `/acmeship/v1`, matching how `acme-server` mounts them
/// (`crates/acme-server/src/http/openapi.rs`).
pub struct Acme {
    pub pay: PayClient,
    pub ship: ShipClient,
}

impl Acme {
    /// `base_url` is the sandbox root, e.g. `http://localhost:2263` — no
    /// trailing slash. `pay_secret_key`/`ship_secret_key` are each
    /// service's own bearer token (spec §10.1/§11.1 — the two dialects
    /// don't share credentials); `progenitor`'s generated client has no
    /// concept of the OpenAPI document's `security` block, so the token
    /// is baked into an `Authorization: Bearer …` default header on each
    /// service's own `reqwest::Client` here instead.
    pub fn new(base_url: &str, pay_secret_key: &str, ship_secret_key: &str) -> Self {
        Self {
            pay: PayClient::new_with_client(
                &format!("{base_url}/acmepay/v1"),
                bearer_client(pay_secret_key),
            ),
            ship: ShipClient::new_with_client(
                &format!("{base_url}/acmeship/v1"),
                bearer_client(ship_secret_key),
            ),
        }
    }
}

fn bearer_client(secret_key: &str) -> reqwest::Client {
    let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {secret_key}"))
        .expect("secret key is valid header value");
    value.set_sensitive(true);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::AUTHORIZATION, value);
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .expect("reqwest client builds")
}
