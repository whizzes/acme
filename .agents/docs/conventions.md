# Working conventions for this repo

Not from the spec — operational rules for agents (and humans) working in
this codebase.

## Agents don't run build commands

**Build, run and test commands are executed by the developer, not the
agent.** Don't run `cargo build`, `cargo run`, `cargo check`, `cargo test`,
or `cargo nextest run` yourself. Write correct code by reading the crate
docs/source and reasoning carefully — you can't lean on the compiler as a
feedback loop here. Consequences:

- Avoid `sqlx::query!`/`query_as!` (compile-time DB/offline-cache
  verification) until a `.sqlx` cache is committed and current — use the
  runtime `sqlx::query()`/`sqlx::query_as::<_, T>()` API instead. See
  [[tech-stack]].
- Double-check axum/maud/sqlx/tower-http API shapes against the pinned
  major version in `Cargo.toml` before using them — minor API shape (e.g.
  `axum::serve` taking a bare `Router` vs. `.into_make_service()`) has
  changed across versions.
- If a change needs verification (does it compile, does the dashboard
  render correctly), ask the developer to run it and report back, or
  describe explicitly what to run.

## Tests: nextest, not `cargo test`

`just test` runs `cargo nextest run --workspace --all-features`. Don't add
doctests as the only coverage for something important — nextest does not
run doctests (`cargo test --doc` still would, separately, but it's not part
of this project's test command).

## Justfile is the source of truth for commands

Every command a developer runs regularly belongs in `Justfile`, not just in
a README. `just --list` should be enough to discover them. Some recipes
(`seed`, `reset`, `openapi`) reference CLI subcommands the M0 binary doesn't
implement yet — they document the target interface from spec §19 ahead of
the milestone that wires them up; don't remove them, implement them when
their milestone lands.

## Cargo workspace

`crates/acme-server` is the only member as of M0. Root `Cargo.toml`'s
`[workspace.dependencies]` centralizes every crate/version from spec §3 —
add new members (`acme-client`, `acme-cli`, `xtask`) at the milestone the
spec calls for (§22), reference existing pinned versions via
`dep = { workspace = true }` rather than re-picking a version. See
[[architecture]] for the rationale.

## Docs layout

- `.agents/docs/*.md` — full reference material distilled from
  `specs/000-Initial-Spec.md`, safe to read instead of re-reading the whole
  spec for routine work.
- `.agents/skills/software-engineer/` — the skill an agent loads to work in
  this repo; `SKILL.md` is the short entry point, `refs/*.md` are scoped
  checklists that point back into `.agents/docs/`.
- `AGENTS.md` (repo root) — the top-level entry point every agent sees
  first; short, points here.

Keep `.agents/docs/*.md` in sync with the spec when the spec changes —
these are a cache, not a fork. If a doc and the spec disagree, the spec
wins; fix the doc.
