# AGENTS.md

`acme` — a mock ecommerce services sandbox (five fake payment gateways,
five fake carriers, one operations dashboard). Full spec:
`specs/000-Initial-Spec.md`. Full distilled reference: `.agents/docs/`.
Skill for working in this repo: `.agents/skills/software-engineer/SKILL.md`
— load it before making non-trivial changes.

## Status

M0 (skeleton) is done: workspace scaffold, config, tracing, SQLite pool +
pragmas, migration wiring, `/healthz`, static file serving, Maud dashboard
shell with a static clock header. See `.agents/docs/delivery-plan.md` for
what's next (M1: full schema + state machines + sim clock + ticker).

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
  The developer runs all build/test commands. Write correct code by
  reading, not by iterating on compiler feedback. Consequence: avoid
  `sqlx::query!`/`query_as!` (need a live DB or offline cache to verify at
  compile time) — use the runtime `sqlx::query()`/`query_as::<_, T>()` API.
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
- **Static file serving path**: `ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/static"))`
  resolves relative to the crate's manifest dir at compile time, so `just
  run` (or any `cargo run` invocation) serves `crates/acme-server/static`
  correctly regardless of the shell's current working directory. A
  runtime-relative path (`"crates/acme-server/static"` or `"static"`) would
  break depending on whether the command runs from the workspace root or
  the crate dir.
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
  handlers, distinct from the per-dialect `AcmeError` enum that lands with
  providers in M2. Don't merge the two when that lands.
- `Config::clock_epoch` is a plain `DateTime<Utc>`, not `Option<...>`.
  `Config::default()` seeds it with `Utc::now()`, but `Default::default()`
  only runs once per `Config::load()` call (inside
  `Figment::from(Serialized::defaults(...))`), so the timestamp is captured
  once at load time, not re-evaluated on every access. An `ACME_CLOCK_EPOCH`
  env var or `acme.toml` value overrides it via the normal merge.
