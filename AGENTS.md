# AGENTS.md

`acme` — a mock ecommerce services sandbox (five fake payment gateways,
five fake carriers, one operations dashboard). Full spec:
`specs/000-Initial-Spec.md`. Full distilled reference: `.agents/docs/`.
Skill for working in this repo: `.agents/skills/software-engineer/SKILL.md`
— load it before making non-trivial changes.

## Status

M0 and M1 are done. M0: workspace scaffold, config, tracing, SQLite pool +
pragmas, `/healthz`, static file serving, Maud dashboard shell. M1: full
schema (spec §7) plus the traffic inspector schema (§21.2, pulled forward
by §21.14); `domain::{ids,money,address,event,payment,shipment,error}`;
`sim::{clock,ticker}`; `capture::{layer,recorder,redact,trace}` (inbound
only — not yet mounted on any route, since no provider router exists until
M2); `AcmeError` (rendered in Acme Pay's own envelope only, for now). See
`.agents/docs/delivery-plan.md` for what's next (M2: Acme Pay + Acme Ship,
idempotency, pricing, scenarios).

## Structure

Cargo workspace. `crates/acme-server` is the only member and builds the
`acme` binary. Shared dependency versions live in root `Cargo.toml`'s
`[workspace.dependencies]`. See `.agents/docs/architecture.md` for the full
layout and the reasoning for having a workspace this early (spec's own
`crates/` restructure is normally an M8 concern).

## Specs

Every feature spec under `specs/` (numbered `NNN-Name.md`, following
`001-Scaffolding.md`) must follow `specs/SPEC_TEMPLATE.md` — same section
order, same headings. Don't modify `SPEC_TEMPLATE.md` itself; copy its
structure into the new numbered file. `000-Initial-Spec.md` predates the
template and is the exception, not a second pattern to follow.

## Hard rules

- **The agent does not run `cargo build`/`run`/`test`/`check`/`clippy`.**
  Tests and builds run locally on the developer's machine, never inside
  Claude/the agent's own sandbox — and separately in GH Actions CI
  (`.github/workflows/ci.yml`: fmt, clippy, test, build). Write correct
  code by reading, not by iterating on compiler feedback. Consequence:
  avoid `sqlx::query!`/`query_as!` (need a live DB or offline cache to
  verify at compile time) — use the runtime `sqlx::query()`/
  `query_as::<_, T>()` API.
- **Tests run with `cargo nextest run`** (`just test`), not `cargo test`.
- **Common commands go in `Justfile`.** `just --list` is the menu.
- **Nothing under `providers/` (from M2 on) touches `sqlx::query*`
  directly** — adapters call into `domain`/`db::repo`.
- Money: integer minor units + ISO-4217, never float. Timestamps: RFC3339
  text, UTC. IDs: prefixed ULIDs.

## Code comments

Keep Rust source close to comment-free. Allowed:

- `///` doc comments on public items (functions, structs, ...) — one or two
  lines, *what* it does or a genuinely non-obvious invariant. Not *why* it
  was built that way.
- `//!` module-level doc comment at the top of a file, stating the module's
  purpose in a line or two.
- Markdown files — `.agents/docs/`, this file, or a per-crate `README.md`
  if a crate accumulates enough implementation-specific rationale to
  warrant one — for design rationale, trade-offs, "why this and not that,"
  gotchas.

Not allowed: inline `//` comments explaining reasoning inside function
bodies, multi-paragraph doc comments, comments that restate what the code
already says. If a decision needs explaining, it goes in a doc file (see
Learnings below for the running log) and gets linked from there, not
inlined next to the code.

## Learnings from implementing M0

- **`axum::serve` takes a bare `Router`, not `.into_make_service()`** in
  axum 0.8 — `Router` implements the service bound `axum::serve` needs
  directly. `.into_make_service()` is the pre-0.7/hyper-server-era pattern;
  don't reintroduce it.
- **SQLite pragmas must be set via `SqliteConnectOptions` builder methods
  (`.journal_mode()`, `.synchronous()`, `.foreign_keys()`,
  `.busy_timeout()`, `.pragma("temp_store", "MEMORY")`), not a one-off
  `PRAGMA ...; execute(&pool)` call.** The pool opens multiple connections
  lazily; executing pragmas against `&pool` only touches whichever single
  connection is checked out for that call, so later-opened connections
  wouldn't get `synchronous`/`foreign_keys`/`busy_timeout`/`temp_store` (all
  per-connection settings). `journal_mode = WAL` is persisted at the DB
  file level so it's less fragile, but the builder-options approach covers
  all five pragmas from spec §7 correctly regardless.
- **Static assets are `rust_embed`-embedded, not `ServeDir`-served.** The
  original `ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/static"))`
  baked in a compile-time *path* — correct for `cargo run`/`just run` on
  the machine that compiled the binary, but the packaged Docker image
  (`docker/Dockerfile`) ships only the `acme` binary, so that path never
  existed in the container and every `/static/*` request 404'd. Fixed by
  deriving `RustEmbed` on a `static/`-folder struct
  (`crates/acme-server/src/web/mod.rs`): debug builds still read `static/`
  off disk at runtime (identical dev behavior), release builds — what
  `just build-release`/the Docker image ship — bake the file bytes into
  the binary, same trick `sqlx::migrate!("./migrations")` already uses for
  migrations. Don't reintroduce a disk-backed path for anything shipped in
  the release binary.
- **`sqlx::migrate!("./migrations")`** resolves relative to
  `CARGO_MANIFEST_DIR` of the crate it's invoked in (compile-time, via the
  macro), so it correctly finds `crates/acme-server/migrations` regardless
  of runtime CWD — no extra path handling needed, unlike `ServeDir`.
- **`figment::providers::Env::prefixed("ACME_")`** matches env vars
  case-insensitively against the (already-snake_case) struct field names,
  so `ACME_DATABASE_URL` → `database_url` without any custom key mapping.
- Declaring the *entire* spec §3 tech stack in root `[workspace.dependencies]`
  up front (including crates no crate uses yet, e.g. `utoipa`, `reqwest`,
  `insta`) is safe and free — Cargo only resolves/fetches a
  `workspace.dependencies` entry once some member's `[dependencies]`
  actually references it with `workspace = true`. This let M0 centralize
  versions for all future milestones without pulling in unused deps today.
- `AppError` (anyhow → 500 in `error.rs`) is unused by any M0 route yet
  (`#[allow(dead_code)]`'d) — it's the fallback for the dashboard's own
  handlers, distinct from `AcmeError`. Don't merge the two.
- `Config::clock_epoch` is a plain `DateTime<Utc>`, not `Option<...>`.
  `Config::default()` seeds it with `Utc::now()`, but `Default::default()`
  only runs once per `Config::load()` call (inside
  `Figment::from(Serialized::defaults(...))`), so the timestamp is captured
  once at load time, not re-evaluated on every access. An `ACME_CLOCK_EPOCH`
  env var or `acme.toml` value overrides it via the normal merge.

## Learnings from implementing M1

- **Added `src/lib.rs`.** Spec §5's layout is `main.rs` only, but
  `tests/*.rs` integration tests can't see a binary crate's internals —
  they compile against the crate as an external dependency, which requires
  a library target. `lib.rs` now owns every module and `pub async fn
  run(cfg)`; `main.rs` is a thin wrapper that loads `Config`, sets up
  tracing, and calls `acme_server::run(cfg)`. Cargo auto-derives the lib
  crate name `acme_server` from the package name `acme-server`
  (`-` → `_`); `[lib] name = "acme_server"` in `Cargo.toml` makes that
  explicit rather than relying on the default silently.
- **`axum::body::to_bytes(body, limit)` replaces `http-body-util`.** It's a
  stable axum re-export (buffers a body up to `limit`, erroring past it) —
  no need for the `http-body-util` dependency the M1 spec assumed; dropped
  it from `[workspace.dependencies]`.
- **`#[derive(sqlx::Type)]` on a fieldless enum with `#[sqlx(rename_all =
  "snake_case")]`** generates `Type`+`Encode`+`Decode` together, mapping to
  a `TEXT` column via the variant name. Used for `PaymentStatus` and
  `ShipmentStatus` directly — far lower-risk than hand-writing `Encode`/
  `Decode` (which is why `domain::ids`' prefixed ULIDs deliberately do
  *not* get a custom `sqlx::Type` impl; they round-trip as plain `String`
  through `Display`/`FromStr` at the repo boundary instead).
- **`MatchedPath` is empty from a middleware mounted via `Router::layer()`.**
  It's populated by the router's own routing step, which runs *inside*
  `next.run()` — a layer wrapping the whole router runs before that and
  never sees it. The fix (`route_layer()` instead) stops 404s from
  reaching the middleware at all, which spec §21.3 explicitly requires
  capturing — so `capture::layer::record_exchange` keeps `Router::layer()`
  and just leaves `route_pattern` `None` for now. Revisit only once the
  dashboard (M3) needs it enough to justify a different mount shape (e.g.
  `route_layer()` per matched route, called out explicitly for 404s
  separately).
- **A `tokio::task_local!` value is only visible inside its `.scope(...)`
  future.** `capture::trace::take_resources()` has to run *inside* the same
  `trace::scope(trace_id, async { .. })` block as `next.run(req)`, not
  after `.await`-ing it — reading it afterward silently returns empty
  (the "not set" error path, swallowed by `unwrap_or_default()`). Wrap
  both the handler call and the read in one `async {}` block passed to
  `scope`.
- **`sim::ticker` never persists a shipment's happy-path schedule.** It
  regenerates `domain::shipment::happy_path_schedule(eta, seed)`
  deterministically every tick from `(shipments.created_at,
  shipments.eta_at, a hash of shipments.id)`, and uses
  `COUNT(shipment_events)` as the index into it. `std::collections::
  hash_map::DefaultHasher` is deterministic for identical input *within a
  build* (unlike `RandomState`, which is what makes `HashMap` iteration
  order random) — good enough here since the schedule only ever needs to
  agree with itself across ticks of the same running process.
- **`domain::shipment::happy_path_schedule`'s command sequence follows the
  state machine (`target()`), not spec §8.2's worked example verbatim.**
  The example's timeline goes `picked_up → at_facility` directly; the
  abstract diagram earlier in the same section goes `picked_up → in_transit
  → at_facility`. They disagree. The schedule generator has to emit only
  legal edges (`apply` would reject anything else), so it follows the
  diagram; see the doc comment on `happy_path_schedule` for the full hop
  list.
- **M2's "mount the capture layer" is one `.layer(...)` call, not new
  design work.** `capture::layer::record_exchange` and everything it
  depends on (`Recorder`, `redact`, `trace`) is fully built and tested
  against a synthetic router (`tests/capture.rs`) with zero knowledge of
  any real provider — that was the point of pulling §21 into M1 (§21.14).
