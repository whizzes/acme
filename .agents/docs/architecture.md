# Architecture

Source: specs/000-Initial-Spec.md §4, §5. One binary, `acme`, built from the
`acme-server` crate in a Cargo workspace (workspace restructure pulled
forward from spec §22.1 into M0 — see [[conventions]]).

## Shape

```
merchant integration ──► provider adapters (dialect in / dialect out, no business logic)
                              │
                              ▼
                        core domain (payment SM · shipment SM · pricing ·
                        scenarios · idempotency · events)
                              │              │
                        ┌─────▼─────┐  ┌─────▼──────┐
                        │ SQLite    │  │ sim engine │
                        │ (WAL)     │◄─┤ clock/ticker/faults/latency │
                        └─────┬─────┘  └─────┬──────┘
                              │              │
                        ┌─────▼─────┐  ┌─────▼──────────┐
      browser (HTMX) ◄──┤ dashboard │  │ webhook         ├──► merchant callback URLs
                        │ Maud+HTMX │  │ dispatcher      │
                        └───────────┘  └─────────────────┘
```

## The one rule that keeps the architecture honest

**Nothing under `providers/` may write to the database directly.** Adapters
parse a dialect into a core command, call `domain`/`db::repo`, and render the
core result back into the dialect. Spec calls for an architecture test that
greps for `sqlx::query` under `src/providers/` and fails the build if found
— add it in M2 when `providers/` first appears.

## Background tasks

Three long-lived tasks, each holding a `CancellationToken` awaited during
graceful shutdown:

| Task | Period | Responsibility | Status |
|---|---|---|---|
| `sim::ticker` | 1s wall clock | Advance sim clock, run due state transitions, emit events | **M1**, shipments only — payments have nothing to schedule until M2 adds scenarios |
| `webhooks::dispatcher` | 1s wall clock | Pick up due deliveries, POST, record attempt, schedule retry | M4 |
| `housekeeping` | 60s wall clock | Expire checkout sessions/idempotency keys, trim retention limits, `PRAGMA wal_checkpoint` | Unscheduled — see spec §21.7 |

`sim::ticker::spawn` is cancelled and awaited in `lib.rs::run`'s shutdown
path; `capture::recorder`'s writer task (also spawned there) is not — see
[[conventions]] / `AGENTS.md` Learnings.

## Repository layout (workspace form)

```
acme/
├── Cargo.toml                  # [workspace], shared [workspace.dependencies]
├── Justfile
├── acme.example.toml
├── AGENTS.md
├── .agents/
│   ├── docs/                   # this directory — full reference material
│   └── skills/software-engineer/
├── specs/000-Initial-Spec.md
└── crates/
    └── acme-server/            # everything from spec §5, under crates/
        ├── Cargo.toml           # [lib] name = "acme_server" + [[bin]] name = "acme"
        ├── migrations/
        ├── static/
        ├── tests/               # integration tests (need the lib target — see below)
        └── src/
            ├── lib.rs           # pub mod tree + `run(cfg)` — main.rs just calls into it
            ├── main.rs          # thin: load Config, init tracing, acme_server::run(cfg)
            ├── config.rs
            ├── state.rs         # AppState { db, cfg, clock, recorder }
            ├── error.rs         # AppError (dashboard) + AcmeError (spec §8.5)
            ├── domain/          # ids, money, address, event, payment, shipment, error
            ├── sim/             # clock, ticker
            ├── capture/         # layer, recorder, redact, trace — inbound only until M4
            ├── db/
            │   ├── mod.rs       # pool, pragmas, migrate, bootstrap_sim_clock
            │   └── repo/        # the only code allowed to run sqlx::query*
            └── web/             # the dashboard
                ├── layout.rs  pages/
```

`src/lib.rs` is a deviation from spec §5's literal `main.rs`-only layout —
added in M1 because `tests/*.rs` integration tests can't reach a binary
crate's internals; see `AGENTS.md` Learnings. `providers/`, `webhooks/`,
`http/`, `seed/` land in later milestones per spec §5.
`crates/acme-client/`, `crates/acme-cli/` and `crates/xtask/` are added at
the point spec §22 describes (client crate generation), not before — don't
scaffold them speculatively.

## Why the workspace exists from M0

Spec §22.1 only mandates the `crates/` restructure starting at M8, when
`acme-client` is extracted. This project pulls it forward to M0 because:

- One `Cargo.lock` and one `target/` dir means a shared dependency (e.g.
  `serde`, `tokio`) compiles once, not once per crate-that-would-otherwise-exist.
- `[workspace.dependencies]` centralizes every version from spec §3 now, so
  adding `acme-client`/`acme-cli`/`xtask` later is "add a member," not
  "renegotiate versions."
- No functional difference to `cargo run` — the binary is still `acme`,
  still built with one command, still zero-Docker-required.
