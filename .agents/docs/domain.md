# Core domain

Source: specs/000-Initial-Spec.md §8. **Not implemented yet** — this is the
M1 deliverable ("both state machines with exhaustive tests; sim clock +
ticker; event log; request-logging middleware; error enum", see
[[delivery-plan]]).

## 8.1 Payment state machine

```
                     ┌──────────► expired
                     │
created ─► pending ──┼──► authorized ──► captured ──┬──► partially_refunded ──► refunded
   │         │       │        │              │      │
   │         │       │        │              │      └──► disputed ──► charged_back
   │         │       └──► rejected           │                 │
   │         │                               │                 └──► dispute_won (→ captured)
   │         └──► cancelled                  └──► cancelled (void before settlement)
   └──► rejected
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum PaymentStatus {
    Created, Pending, Authorized, Captured,
    PartiallyRefunded, Refunded,
    Rejected, Cancelled, Expired,
    Disputed, ChargedBack,
}

impl PaymentStatus {
    pub fn can_transition_to(self, next: Self) -> bool { /* table lookup */ }
    pub fn is_terminal(self) -> bool { /* … */ }
}

pub enum DeclineReason {
    InsufficientFunds, CardExpired, InvalidCvv, DoNotHonor, StolenCard,
    LimitExceeded, IssuerUnavailable, RiskRejected, ThreeDsFailed, Timeout,
}
```

Illegal transitions return `409 Conflict` in the dialect's own error shape,
never a panic. The transition function is pure and unit-tested exhaustively:

```rust
pub fn apply(cur: PaymentStatus, cmd: PaymentCommand, clock: &SimClock)
    -> Result<Transition, DomainError>;
```

`Transition` carries the new status, the events to emit, and an optional
`next_transition_at` for the ticker.

## 8.2 Shipment state machine

```
quoted ─► created ─► label_generated ─► picked_up ─► in_transit ⇄ at_facility
                                                          │
                                                          ▼
                                                  out_for_delivery
                                                    │    │    │
                              delivery_attempted ◄──┘    │    └──► exception
                                    │  (attempts < 3)    │            │
                                    └────────────────────┘            ▼
                                                          ┌───► returning ─► returned
                                    delivered ◄───────────┘
                                                          └───► lost
cancelled  ← from created | label_generated | picked_up
```

```rust
pub enum ShipmentStatus {
    Quoted, Created, LabelGenerated, PickedUp, InTransit, AtFacility,
    OutForDelivery, DeliveryAttempted, Delivered,
    Exception, Returning, Returned, Cancelled, Lost,
}
```

The **happy path generator** takes a service level and produces the full
event schedule up front, in sim time, with plausible facility names drawn
from a geography table. Timings jitter ±25% from a seeded RNG so two
shipments never look identical; each transition writes a `shipment_events`
row plus an outbound webhook event.

## 8.3 Simulation clock

```rust
pub struct SimClock {
    epoch: DateTime<Utc>,        // simulated time at start
    started_at: Instant,         // wall clock at start
    multiplier: AtomicU32,       // fixed point, x1000
    paused: AtomicBool,
    frozen_at: Mutex<Option<DateTime<Utc>>>,
}

impl SimClock {
    pub fn now(&self) -> DateTime<Utc>;
    pub fn set_multiplier(&self, m: f64);   // 0.0 (paused) … 3600.0 (1s = 1h)
    pub fn jump(&self, d: Duration);        // "advance 6 hours" button
    pub fn reset(&self, to: DateTime<Utc>);
}
```

Every timestamp written to a provider response is sim time. Log lines and
`api_requests.ts` use wall time — the dashboard shows both. UI presets:
**Paused · Real time · 1 min/s · 1 h/s · 1 day/s**.

M0's `Config::clock_epoch` is the seed for this struct's `epoch` field, but
M0 only renders it as static text in the dashboard header
(`web::pages::overview`) — no `SimClock` type exists yet, no ticker runs.

## 8.4 Idempotency

Applies to every non-`GET` provider route. Key source per dialect:
`Idempotency-Key` (Chargeflow), `X-Idempotency-Key` (Pago Rápido),
`Acme-Idempotency-Key` (Acme Pay and Acme Ship), none for Trancorp Webpay
(naturally idempotent by `buy_order`).

1. Hash the canonicalized request body (sorted keys, whitespace-stripped).
2. `INSERT … ON CONFLICT DO NOTHING` into `idempotency_keys` with `state='in_flight'`.
3. Insert won → execute, store `(status, body)`, set `state='complete'`.
4. Insert lost, stored hash matches → replay stored response, add
   `Acme-Idempotent-Replay: true` header, mark the request log row.
5. Insert lost, hash differs → `409`/`400` in dialect shape: "idempotency
   key reused with a different body".
6. Insert lost, still `in_flight` → `409` with a retry hint.

Keys expire after 24h of **sim** time.

## 8.5 Errors

One internal error enum, rendered per dialect:

```rust
pub enum AcmeError {
    Unauthorized(&'static str), Forbidden, NotFound(&'static str),
    Validation(Vec<FieldError>), Conflict(String), UnprocessableState { from: String, to: String },
    RateLimited { retry_after_s: u64 }, ProviderDown, Injected(FaultId), Internal(anyhow::Error),
}
```

| Dialect | Shape |
|---|---|
| Acme Pay / Acme Ship | `{"error":{"type":"validation_error","code":"amount_too_small","message":"…","param":"amount","request_id":"req_…","doc_url":"…"}}` |
| Trancorp Webpay | `{"error_message":"Invalid value for parameter 'buy_order'"}` with 422 |
| Pago Rápido | `{"message":"…","error":"bad_request","status":400,"cause":[{"code":2034,"description":"…"}]}` |
| Chargeflow | `{"error":{"type":"invalid_request_error","code":"parameter_missing","param":"line_items","message":"…","request_log_url":"…"}}` |
| Nordika | RFC 9457 problem details: `{"type":"https://…/errors/insufficient-limit","title":"…","status":402,"detail":"…","instance":"/nordika/v2/orders/…"}` |
| Iberex | `{"resultado":"KO","codigoError":"E-0231","mensaje":"CCC no autorizado para el servicio"}` |
| Postalis | `{"estado":"ERROR","codigoRetorno":"1104","descripcionRetorno":"Código postal no válido"}` |
| ShipHub | `{"meta":"rate","error":{"code":"CARRIER_NOT_AVAILABLE","message":"…"},"data":[]}` |
| Veloz | `{"ok":false,"error":{"kind":"out_of_zone","hint":"…","support_id":"req_…"}}` |

`request_id` is always `api_requests.id` — an error message in a customer's
terminal is a deep link into the dashboard: `/requests/req_01JAV…`.

M0 has its own tiny, unrelated `error::AppError` (anyhow → 500) for the
dashboard's own routes — not this enum. Don't conflate the two when M2 adds
provider adapters.

## 8.6 Request logging middleware

A `tower` layer wraps every **provider** route (not the dashboard):

1. Assign `req_…` id, put it in the tracing span and `Acme-Request-Id` header.
2. Buffer the request body up to `ACME_REQUEST_LOG_BODY_LIMIT`, redacting
   anything matching `card_number|cvv|card[.\[]number|secret|password` to
   `«redacted»`.
3. Check `sim_faults` for a match; short-circuit with injected behaviour if
   one fires, record `fault_injected`.
4. Apply configured latency (global + fault-specific).
5. Run the handler, buffer the response, write the row, broadcast a compact
   summary on the SSE channel.

Writes go through a bounded `mpsc` channel to a single writer task — a burst
of traffic never blocks a handler on the SQLite write lock. If the channel
is full, the log row is dropped and a counter increments: logging must never
be the reason a request fails.

This is one half of the traffic inspector (spec §21); the webhook dispatcher
is the other half, sharing the `http_exchanges` table.
