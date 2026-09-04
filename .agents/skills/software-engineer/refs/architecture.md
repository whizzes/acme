# Architecture — quick reference

Full doc: `.agents/docs/architecture.md`. Source: spec §4, §5, §22.1.

## Layout

```
acme/
├── Cargo.toml            # [workspace], [workspace.dependencies]
├── Justfile
├── AGENTS.md
├── .agents/
├── specs/000-Initial-Spec.md
└── crates/
    └── acme-server/       # lib `acme_server` + bin `acme`
        ├── Cargo.toml
        ├── migrations/
        ├── static/
        ├── tests/          # integration tests — need the lib target
        └── src/
            ├── lib.rs      # pub mod tree + run(cfg); main.rs just calls it
            ├── main.rs     # thin: Config::load, tracing, acme_server::run
            ├── config.rs
            ├── state.rs
            ├── error.rs    # AppError (dashboard) + AcmeError (spec §8.5)
            ├── domain/      # ids, money, address, event, payment, shipment
            ├── sim/         # clock, ticker
            ├── capture/     # layer, recorder, redact, trace — inbound only
            ├── db/          # pool, pragmas, migrate, repo/
            └── web/         # dashboard: layout.rs, pages/
```

Later milestones add `providers/`, `webhooks/`, `http/`, `seed/` under
`crates/acme-server/src/`, matching spec §5. `crates/acme-client/`,
`acme-cli/`, `xtask/` are M8+ (spec §22) — don't add them early.

## The rule

**Nothing under `providers/` may write to the database directly.** A
provider adapter's job: parse dialect request → core domain command → call
`domain`/`db::repo` → render core result back into the dialect's response
shape. If you're inside `providers/` and reaching for `sqlx::query`, stop —
the query belongs in `db::repo`, called from `domain`.

## Background tasks (from M1)

| Task | Period | Responsibility |
|---|---|---|
| `sim::ticker` | 1s wall | Advance sim clock, run due transitions, emit events |
| `webhooks::dispatcher` | 1s wall | Pick up due deliveries, POST, record, retry |
| `housekeeping` | 60s wall | Expire sessions/idempotency keys, trim logs, WAL checkpoint |

Each holds a `CancellationToken`, awaited during graceful shutdown — follow
the pattern already in `main.rs`'s `shutdown_signal()` for any new
long-lived task.

## Why the workspace exists already (M0, not M8)

Spec §22.1 only mandates `crates/` at M8. Pulled forward so
`[workspace.dependencies]` centralizes every crate version from spec §3
now — adding `acme-client`/`acme-cli`/`xtask` later is "add a member and
reference `workspace = true`," not "renegotiate every version." No
behavior change to `cargo run`.
