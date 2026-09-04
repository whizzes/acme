# Tech stack

Source: specs/000-Initial-Spec.md §3. Full crate list and pinned major
versions live in the root `Cargo.toml` under `[workspace.dependencies]` —
this file explains *why*, the table there says *what version*.

| Concern | Crate | Notes |
|---|---|---|
| HTTP server | `axum` | Path params use `{id}` syntax (0.8 changed from `:id`) |
| Async runtime | `tokio` | `rt-multi-thread`, `macros`, `signal`, `time` |
| Middleware | `tower`, `tower-http` | `trace`, `compression-br`, `fs`, `limit`, `timeout` |
| Templating | `maud` | `axum` feature — `Markup: IntoResponse` |
| Database | `sqlx` | `sqlite`, `runtime-tokio`, `migrate`, `chrono`, `macros` |
| OpenAPI | `utoipa` + `utoipa-axum` + `utoipa-swagger-ui` | Handler and spec cannot drift; one `OpenApi` doc per provider |
| Serialization | `serde`, `serde_json` | |
| Form dialects | `serde_qs` | Stripe-style `a[0][b]=c` bodies |
| Fake data | `fake` | Seeder |
| RNG | `rand` | Seeded `StdRng` for reproducible fixtures |
| Time | `chrono` | `serde` feature; RFC3339 **text** in SQLite, never a numeric column |
| IDs | `ulid` | Sortable, prefixed (`pay_01J…`) |
| Crypto | `hmac`, `sha2`, `base64`, `hex` | Webhook signatures only |
| HTTP client | `reqwest` | `json`, `rustls-tls` — outbound webhooks |
| Logging | `tracing`, `tracing-subscriber` | `env-filter` |
| Errors | `thiserror` (library code), `anyhow` (`main`) | |
| Config | `figment` | `ACME_`-prefixed env vars over optional `acme.toml` |
| Tests | `insta`, `axum-test`, `cargo-nextest` | Snapshot the OpenAPI doc and HTML fragments; **run tests with nextest, not `cargo test`** |

## Why these choices

- **Maud over Askama/Tera**: templates are Rust — a renamed status enum is a
  compile error, not a blank `<span>`. Fragments compose as functions, which
  is what HTMX wants.
- **SQLite over Postgres**: the whole product is `git clone && cargo run`.
  WAL mode plus a single writer task is enough. Schema avoids anything
  Postgres-only.
- **utoipa-axum over hand-written YAML**: handler and spec cannot drift.

## sqlx macro caution

`sqlx::query!`/`query_as!` compile-time-verify against a live database or a
committed `.sqlx` offline cache. Neither exists yet at M0. Until a
`cargo sqlx prepare` cache is committed, write queries with the runtime
`sqlx::query()` / `sqlx::query_as::<_, T>()` API, not the `!` macros — an
agent editing this repo cannot run `cargo build` to regenerate the cache
(see [[conventions]]), so a `!`-macro query with a stale/missing cache would
silently block the next `cargo build` the dev runs.
