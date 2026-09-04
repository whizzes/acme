# Delivery plan

Source: specs/000-Initial-Spec.md §20.

| Milestone | Scope | Done when |
|---|---|---|
| **M0 — Skeleton** (½ day) — **done** | Cargo project, config, tracing, SQLite pool + pragmas, migrations, `/healthz`, static file serving, Maud layout with the clock header rendering a static time | `cargo run` serves a styled empty dashboard |
| **M1 — Core** (2 days, +½ day for §21.14) — **done** | Full schema (§7) + traffic inspector schema (§21.2); `domain::{ids,money,address,event,payment,shipment,error}`; `sim::{clock,ticker}`; inbound `capture::{layer,recorder,redact,trace}`; `AcmeError` (house-dialect rendering only) | Ticker advances a hand-inserted shipment through to delivered; state machine tests green; see specs/002-Core.md |
| **M2 — First vertical slice** (2 days) | Acme Pay + Acme Ship end to end; idempotency; pricing engine; scenarios; utoipa for both; Swagger UI | `tests/scenarios.rs` passes for one merchant; Swagger "Try it out" creates a payment |
| **M3 — Dashboard** (2 days) | Overview, payments, shipments, requests, detail pages, filters, SSE live feed, advance/refund actions | An engineer can debug an integration without reading the database |
| **M4 — Webhooks** (1½ days) | Endpoints, dispatcher, all signature schemes, retries, delivery UI with signed-string display and verification snippets | Webhook tests green; a failing endpoint visibly retries and auto-disables |
| **M5 — Dialects** (4 days, parallelizable) | Trancorp Webpay, Pago Rápido, Chargeflow, Nordika; Iberex, Postalis, ShipHub, Veloz; hosted checkout pages; per-provider specs | Every provider has a passing snapshot integration test and appears in Swagger |
| **M6 — Simulation** (1½ days) | Fault rules, latency and failure sliders, chaos UI, manual transitions, public tracking pages | A demo can produce a 503, a decline, and a lost parcel on request |
| **M7 — Seed and polish** (1½ days) | Faker world, scale presets, empty states, keyboard shortcuts, dark mode, README, `.http` files, Docker, metrics | Fresh clone to a populated, moving dashboard in one command |

M0–M3 is the useful minimum, about a week for one engineer. Roughly three
weeks total, less with M5 split across two.

This project pulled the **workspace restructure** (spec §22.1, normally an
M8 concern when `acme-client` is extracted) forward into M0 — see
[[architecture]] for why. `acme-client`, `acme-cli` and `xtask` are still
M8+ work; don't scaffold them before M5–M7 land.

## Definition of done for a provider (M2/M5)

A provider is finished when all eight are true:

1. Every documented endpoint is implemented and appears in that provider's
   OpenAPI spec with a working example.
2. Auth, error envelope, pagination and timestamp format match the dialect,
   including the awkward parts.
3. Status mapping to the normalized vocabulary is a `const` table, rendered
   on the provider page.
4. Idempotency behaves per the dialect (or is documented as absent).
5. Webhooks are signed with the dialect's scheme and verified by a test
   using an independent implementation.
6. All applicable magic values from spec §9 produce the documented outcome.
7. An `insta` snapshot integration test covers create, retrieve, list, the
   failure path, and the terminal state.
8. A `requests/{slug}.http` file exists with a working call for each endpoint.
