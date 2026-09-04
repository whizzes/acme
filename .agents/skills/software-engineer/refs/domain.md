# Domain cheat sheet

Full doc: `.agents/docs/domain.md`, `.agents/docs/providers-and-scenarios.md`,
`.agents/docs/data-model.md`. Source: spec §7, §8, §9. State machines,
clock, event log and `AcmeError` are implemented (M1, `domain/`+`sim/`);
idempotency *behavior* and scenarios are still M2/M5 — the tables exist,
nothing reads them yet.

## Payment status

`Created → Pending → Authorized → Captured → {PartiallyRefunded → Refunded,
Disputed → ChargedBack | dispute_won→Captured}`, with `Rejected`,
`Cancelled`, `Expired` as early exits. Transition function is pure:
`apply(cur, cmd, clock) -> Result<Transition, DomainError>`. Illegal
transition = `409`, never a panic.

## Shipment status

`Quoted → Created → LabelGenerated → PickedUp → InTransit ⇄ AtFacility →
OutForDelivery → {Delivered, DeliveryAttempted (retry <3), Exception →
Returning → Returned, Lost}`. `Cancelled` reachable from `Created`,
`LabelGenerated`, `PickedUp`. Happy-path event schedule is generated up
front in sim time, ±25% jitter from seeded RNG.

## Sim clock

`sim::clock::SimClock` (`Arc<RwLock<{epoch, started_at, multiplier}>>`,
simpler than the spec's atomics sketch). Provider response timestamps are
sim time; log timestamps are wall time. Multiplier range 0.0 (paused, no
separate pause flag needed) – 3600.0 (1s = 1h). Seeded once from
`sim_settings` on boot (`db::bootstrap_sim_clock`); a restart never resets
a dashboard-adjusted clock.

## Idempotency

Non-`GET` provider routes only. Header name varies by dialect (see
`.agents/docs/domain.md` §8.4). Hash canonicalized body → `INSERT ... ON
CONFLICT DO NOTHING` → won: execute+store; lost+hash matches: replay +
`Acme-Idempotent-Replay: true`; lost+hash differs: `409`/`400`; lost+still
in-flight: `409` with retry hint. Keys expire after 24h **sim** time.

## Errors

One `AcmeError` enum, rendered into 9 different dialect shapes (Acme,
Webpay, Pago Rápido, Chargeflow, Nordika/RFC9457, Iberex, Postalis, ShipHub,
Veloz) — see `.agents/docs/domain.md` §8.5 for the exact JSON shape of each.
`request_id` in every error is always `api_requests.id`.

## Scenarios (forcing outcomes)

Precedence: magic values (PAN/amount/email/postal code) > `scenario`
field/header override > dashboard fault rules. Full magic-value tables in
`.agents/docs/providers-and-scenarios.md`. Test PANs are the published
industry-standard test numbers, stored only as brand+last4 — no real card
data ever.

## Data model essentials

Money = integer `_cents` + ISO-4217 code, never float. Timestamps = RFC3339
text UTC. IDs = prefixed ULIDs. `next_transition_at` due-queries are plain
string comparison (`<=`) — only correct because every timestamp is RFC3339
UTC text. Full DDL: `.agents/docs/data-model.md`.
