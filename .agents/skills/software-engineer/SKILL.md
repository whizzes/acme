---
name: software-engineer
description: Conventions and reference docs for implementing acme, the mock ecommerce services sandbox (Rust/Axum/Maud/HTMX/SQLite Cargo workspace). Use when writing, reviewing, or planning code changes anywhere in this repo.
---

# Software engineer — acme

`acme` is a single Rust binary pretending to be five payment gateways and
five last-mile carriers, plus an operations dashboard. Full product spec:
`specs/000-Initial-Spec.md`. Full distilled reference: `.agents/docs/`.

This skill is the entry point for making code changes here. It doesn't
repeat the spec — it tells you which doc to open for what, and states the
rules that aren't obvious from reading code alone.

## Before writing code

1. Check `.agents/docs/delivery-plan.md` for which milestone the change
   belongs to, and whether the current milestone's prerequisites already
   exist. Don't build M3 dashboard features on top of a M1 domain layer
   that isn't there yet.
2. Load the relevant ref file below instead of re-reading the whole spec.
3. If touching `providers/` (M2+), re-read `refs/architecture.md` — the
   no-direct-DB-access rule is the one thing that's easy to violate by
   accident when an adapter "just needs one lookup."

## Refs (load on demand)

- `refs/architecture.md` — repo/workspace layout, the provider isolation
  rule, background tasks, why the workspace exists from M0.
- `refs/workflow.md` — build/test/lint commands, the "agent doesn't run
  builds" rule, sqlx macro caution, Justfile conventions.
- `refs/domain.md` — payment/shipment state machines, sim clock,
  idempotency, error enum, magic-value scenarios — the cheat sheet for M1/M2
  work.

## Full reference material

`.agents/docs/` has the complete distilled spec: `tech-stack.md`,
`architecture.md`, `data-model.md` (full DDL), `domain.md`, `providers-and-
scenarios.md`, `delivery-plan.md`, `conventions.md`. Refs above are trimmed
pointers into these; go to `.agents/docs/` for the full text, and to
`specs/000-Initial-Spec.md` itself for anything not covered there (e.g. the
per-provider endpoint tables in spec §10/§11, the traffic inspector schema
in §21, the `acme-client` crate design in §22).

## Non-negotiables

- Money is integer minor units + ISO-4217 code. Never a float.
- Timestamps are RFC3339 text, UTC, everywhere in the DB.
- IDs are prefixed ULIDs (`pay_…`, `shp_…`, ...).
- Nothing under `providers/` calls `sqlx::query*` directly.
- Tests run with `cargo nextest run`, wired via `just test` — not
  `cargo test`.
- You (the agent) do not run `cargo build`/`run`/`test`/`check` — see
  `refs/workflow.md`.
