---
name: acme-engineer
description: How to integrate any Rust project with ACME's sandbox ecommerce API (Acme Pay + Acme Ship) — getting a generated client, auth, idempotency, deterministic scenario testing, and webhook signature verification. Use when adding ACME as a dependency or test target in a Rust codebase, inside this repo or a separate one.
---

# Acme engineer — integrating ACME into a Rust project

ACME is a simulated ecommerce sandbox: no real money moves, no real
packages ship. It exposes two of its own services — **Acme Pay**
(`/acmepay/v1`) and **Acme Ship** (`/acmeship/v1`) — plus several
third-party dialects it *emulates* for realism (Trancorp Webpay, Iberex
Express, ...). This skill is about integrating Acme Pay/Ship, ACME's own
API; see `specs/1-Acme-Client.md` §"Should Trancorp Webpay and Iberex
Express be included" for why the emulated dialects are a different
concern.

Full design doc: `specs/1-Acme-Client.md`. Reference implementation:
`crates/acme-client`.

## 1. Get a client

Never hand-write request/response structs against Acme Pay/Ship — both
are fully `utoipa`-documented and spec-lint-clean (every operation has an
`operation_id`, every schema is named, every field described), which
exists specifically so a generator produces a clean client. Use
[`progenitor`](https://github.com/oxidecomputer/progenitor).

**Inside this workspace**: depend on `crates/acme-client` directly
(`acme-client = { path = "../acme-client" }`). It already does step 2-4
below — `Acme::new(base_url, pay_secret_key, ship_secret_key)` returns
`.pay`/`.ship` generated clients with auth baked in.

**In a separate Rust project**: generate your own client from ACME's
committed OpenAPI documents rather than fetching them live:

```toml
[dependencies]
progenitor = "0.15"
progenitor-client = "0.15"
# progenitor-client 0.15 hard-pins reqwest 0.13 (default-features = false,
# features = ["json", "query", "stream"]) — don't also depend on reqwest
# 0.12 in the same crate, Cargo will pull two copies and every generated
# method will fail to type-check against your reqwest::Client.
reqwest = { version = "0.13", default-features = false, features = ["rustls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

```rust
mod pay {
    progenitor::generate_api!(spec = "path/to/openapi.json");
}
mod ship {
    progenitor::generate_api!(spec = "path/to/openapi-ship.json");
}
```

Copy `assets/openapi.json` and `assets/openapi-ship.json` from this repo
— **don't** fetch them from a live server's `/openapi/acmepay.json`/
`/openapi/acmeship.json` and feed those to `generate_api!` directly.
`utoipa` (this server's OpenAPI library) only emits OpenAPI **3.1**;
`progenitor` only accepts **3.0.x** and fails to parse 3.1's
`"type": ["string", "null"]` nullable shape. The committed `assets/*.json`
files are already downgraded to 3.0.3 by `cargo xtask export-openapi`
(`crates/xtask/src/export_openapi.rs`) — regenerate them with that
command if you need a fresher copy, or port that file's `denullify()`
logic if you're scripting your own export from a live instance.

Acme Pay and Acme Ship are two separate documents and two separate
generated `Client` types — `progenitor` generates one client per document
with one base URL, and the two services live at different path prefixes
on the same host, so there's no single merged client to generate.

## 2. Auth

Each service takes its own bearer token in `Authorization: Bearer <key>`
— Acme Pay and Acme Ship credentials are not interchangeable. With
`progenitor`'s default (positional) interface style there's no per-call
`.header(...)` builder, so bake the token into the client's underlying
`reqwest::Client` at construction time via `Client::new_with_client`:

```rust
fn bearer_client(secret_key: &str) -> reqwest::Client {
    let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {secret_key}"))
        .expect("secret key is valid header value");
    value.set_sensitive(true);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::AUTHORIZATION, value);
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}

let pay = pay::Client::new_with_client(&format!("{base_url}/acmepay/v1"), bearer_client(pay_key));
let ship = ship::Client::new_with_client(&format!("{base_url}/acmeship/v1"), bearer_client(ship_key));
```

(`crates/acme-client/src/lib.rs` does exactly this — copy it rather than
reinventing it.)

## 3. Idempotency (Acme Pay only)

Mutating Acme Pay calls (`create_payment`) take an optional
`Acme-Idempotency-Key` header. The same key with the same request body
replays the stored response instead of charging twice; the same key with
a *different* body is a 409. Always set one on any payment-creating call
that might be retried (timeouts, at-least-once queue processing, etc).

## 4. Deterministic testing: force a scenario

Don't rely on the sandbox's magic-value matching (specific card PANs,
amount suffixes, postal codes — spec §9.1/§9.2) for test determinism;
it's a convenience fallback, not the primary integration point. Instead
set the explicit override, which always wins over magic values:

- `X-Acme-Scenario` request header, **or**
- `metadata.acme_scenario` field in the JSON body (header wins if both
  are set).

Value is a snake_case scenario name — full lists in
`crates/acme-server/src/domain/scenario.rs` (`PaymentScenario`,
`ShipmentScenario`). Payments include `approve`,
`decline_insufficient_funds`, `decline_expired_card`, `processing_error`,
`three_ds_challenge`, `chargeback`, `slow_approval`, `manual_review`,
`risk_reject`, and more — read the enum for the exhaustive, current list
rather than trusting a copy of it here.

## 5. Error handling

Every generated operation returns `Result<ResponseValue<T>, Error<AcmeErrorBody>>`.
`Error::ErrorResponse(ResponseValue<AcmeErrorBody>)` is a documented 4xx/5xx
with ACME's typed error envelope (`code`, `message`, `param`,
`request_id`); anything else (`Error::UnexpectedResponse`,
`Error::CommunicationError`, ...) is transport-level or an undocumented
status. Match on the former to branch on ACME-specific failure reasons
(e.g. a decline vs. a validation error); treat the rest as generic
failures.

## 6. Receiving webhooks

If you register a webhook endpoint (`create_webhook_endpoint`), verify
every delivery before trusting its body — ACME signs with HMAC-SHA256,
not a bearer token:

- Header: `Acme-Signature: t=<unix_seconds>,v1=<hex_hmac>`
- Signed string: `"{t}.{raw_request_body}"` (raw bytes as received, before
  any JSON parsing)
- Compare `v1` against your own HMAC-SHA256 of that string, keyed by the
  webhook endpoint's signing secret (issued at registration).

```rust
use hmac::{Hmac, Mac};
use sha2::Sha256;

// `mac.verify_slice` is `subtle`-backed constant-time comparison under the
// hood — prefer it over hex-encoding both sides and comparing with `==`.
fn verify(secret: &str, timestamp: &str, raw_body: &[u8], v1_hex: &str) -> bool {
    let mut signed = format!("{timestamp}.").into_bytes();
    signed.extend_from_slice(raw_body);
    let Ok(expected_bytes) = hex::decode(v1_hex) else {
        return false;
    };
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(&signed);
    mac.verify_slice(&expected_bytes).is_ok()
}
```

Reject deliveries where `t` is too far in the past — pick a tolerance
appropriate to your own retry/clock-skew tolerance, ACME doesn't mandate
one.

## 7. Sim clock: don't assume synchronous terminal states

The sandbox runs its own accelerated clock; a created shipment/payment
can keep transitioning after your call returns (`clock_multiplier`).
Don't assert a terminal status immediately after creation — poll
`get_shipment`/`get_payment`, or better, rely on webhooks for the
state-change notification instead of polling right away.

## Gotchas checklist

- Fetching a live `/openapi/*.json` and feeding it straight to
  `generate_api!` fails — it's OpenAPI 3.1, `progenitor` wants 3.0.x. Use
  `assets/openapi*.json` or replicate `xtask`'s downgrade.
- A `reqwest` version mismatch between your crate and
  `progenitor-client`'s pinned 0.13 produces confusing "two different
  versions of crate `reqwest`" type errors, not a dependency-resolution
  error — check `cargo tree -p reqwest` first if you see that.
- Acme Pay ≠ Acme Ship credentials; don't reuse one bearer token for both.
- Emulated third-party dialects (Trancorp Webpay, Iberex Express) don't
  use ACME's own auth/idempotency/error-envelope/signature conventions —
  they use whatever the real provider they emulate uses. This skill only
  covers Acme Pay/Ship.

## Reference

- `specs/1-Acme-Client.md` — the design this skill implements, including
  the OpenAPI 3.1→3.0 downgrade rationale and why Pay/Ship stay two
  documents instead of one merged spec.
- `crates/acme-client/src/lib.rs` — the reference client wiring (auth,
  two generated clients, one façade).
- `crates/xtask/src/export_openapi.rs` — the exact downgrade logic
  (`downgrade_to_openapi_3_0`/`denullify`) to copy if exporting specs
  yourself.
- `assets/openapi.json`, `assets/openapi-ship.json` — the committed,
  generator-ready specs.
- `crates/acme-server/tests/client.rs` — a full worked example: real
  HTTP round trip through the generated client against a live test
  server.
