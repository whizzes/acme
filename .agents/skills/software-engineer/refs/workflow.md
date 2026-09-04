# Workflow — quick reference

Full doc: `.agents/docs/conventions.md`, `.agents/docs/tech-stack.md`.

## The agent does not run build commands

The developer runs `cargo build`/`run`/`test`/`check`/`clippy` — not you.
Write code carefully by reading source/docs, not by iterating on compiler
errors. Practical implications:

- **No `sqlx::query!`/`query_as!`.** These verify at compile time against a
  live DB or a committed `.sqlx` offline cache; neither is guaranteed
  current when you can't run `cargo build` to check. Use the runtime
  `sqlx::query(...)` / `sqlx::query_as::<_, T>(...)` API and bind params
  with `.bind(...)`.
- Get axum/maud/sqlx/tower-http call shapes right the first time — check
  the pinned major version in `Cargo.toml` (`[workspace.dependencies]`)
  before assuming an API shape from memory; these crates change patterns
  across majors (e.g. axum's `{id}` path syntax since 0.8, `axum::serve`
  taking a bare `Router`, not `.into_make_service()`).
- If something genuinely needs compiling/running to verify (a template
  renders right, a migration applies cleanly), say exactly what command to
  run and ask the developer to run it, rather than running it yourself.

## Commands (all in `Justfile`, run `just --list`)

| Recipe | Does |
|---|---|
| `just run` | `cargo run -p acme-server` with dev log filter |
| `just watch` | same, via `cargo watch` |
| `just test` | `cargo nextest run --workspace --all-features` — **not** `cargo test` |
| `just fmt` | `cargo fmt --all` |
| `just lint` | `cargo fmt --check` + `cargo clippy -D warnings` |
| `just seed` / `reset` / `openapi` | target CLI subcommands from spec §19 — not wired into `main.rs` until their milestone lands; recipes document the intended interface, don't delete them |

Add every new common command to `Justfile`, not just a README — `just
--list` should be the complete menu.

## Workspace dependency pattern

New crate dependency that's already in `[workspace.dependencies]` (root
`Cargo.toml`)? Reference it as `foo = { workspace = true }` in the member
crate's `Cargo.toml`, don't repin a version. Need a crate not yet listed?
Add it to `[workspace.dependencies]` first (with the major version from
spec §3 if it's one of the spec's named crates), then reference it the same
way.
