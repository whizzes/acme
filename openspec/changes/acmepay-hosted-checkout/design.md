## Context

See proposal.md - Why/What Changes for motivation. Relevant existing pieces
this design reuses as-is:

- `checkout_sessions` table already has a nullable `payment_id` and a
  `close_checkout_session(id, payment_id, payment_status, now)` repo
  function built for exactly this "session created open, payment created
  later on submit, then linked" shape — added for Trancorp Webpay
  (`db::repo::payments`), unused by Acme Pay today.
- `providers::payments::acmepay::routes::create_payment` already contains
  the full scenario-resolve -> `payments::create` -> walk
  `scenario::payment_initial_steps` through `payment::apply`/
  `payments::advance` sequence this design needs to reuse verbatim for
  card-present checkout.
- `web::pages::checkout` (Trancorp Webpay's hosted page) is the only
  existing hosted-checkout page and the template for module shape, but its
  flow does not transfer directly: Webpay's page only *records* a forced
  outcome (`set_checkout_session_payment_status`) and redirects; the actual
  payment resource is created later by a separate merchant-initiated
  `PUT .../commit` call, because real Trancorp Webpay's API works that way.
  Acme Pay has no such second merchant call in its dialect (spec §10.1:
  intent + capture, no external commit step) — its hosted page must create
  and close out the payment itself, in the same request that renders the
  outcome to the shopper.
- `webhooks::dispatcher` already polls for due deliveries off
  `payments::advance`-written events; nothing about it is scoped to how a
  payment was created (API vs. hosted page), so it needs no changes.

## Goals / Non-Goals

**Goals:**
- Make `hosted_url` a working page for Acme Pay checkout sessions
  specifically, using the exact same scenario/state-machine code path the
  API already uses (no second implementation of magic-value resolution).
- Keep the generic `/checkout/{token}` `501` stub intact for every provider
  that doesn't have its own hosted page yet.
- Document `acme.toml` in the dashboard without adding new config surface.

**Non-Goals:**
- Chargeflow's own checkout page or the shared `/chargeflow/c/pay/{cs_id}`
  component specs/x7-Additional-Dialects.md describes — out of scope per
  the confirmed scope; only the route *name* (`/acmepay/c/{cs_id}`) is
  chosen now so that slice doesn't need to rename it later.
- Any new `acme.toml`/`Config` key, or a way to pre-register webhook
  endpoints from config — confirmed out of scope; endpoints stay
  per-merchant, API/dashboard-registered.
- Non-card payment methods (PIX/boleto) on the hosted page — the existing
  `create_payment` API already handles those; the hosted page mirrors only
  the card path, matching what Webpay's page does today for its own method.

## Decisions

**Route path and shape: dedicated `/acmepay/c/{cs_id}` (GET renders the
form, POST processes it), not the generic `/checkout/{token}` handler.**
Alternative considered: make the generic `hosted_checkout_placeholder`
dispatch per `provider_slug` instead of adding a new route. Rejected: the
project's own spec (x7-Additional-Dialects.md item 1) already names
`/acmepay/c/{cs_id}` as this page's future path (shared later with
Chargeflow), and Webpay already established the pattern of "each dialect
gets its own dedicated checkout route, the generic one is the not-built-yet
fallback." Matching that avoids a route rename when Chargeflow's slice
lands. `create_checkout_session` changes `hosted_url` to build this path
instead of the generic one.

**Card input is a typed PAN field, not Webpay-style forced-outcome
buttons.** Alternative considered: copy Webpay's three buttons
(approve/reject/abandon). Rejected: Acme Pay's dialect already exposes a
rich set of magic values keyed off the actual PAN, amount, and email
(§9.1) — a shopper (or a demo operator) typing `4000 0000 0000 3220` to
exercise the 3-D-Secure-challenge path is the same mental model the direct
API already teaches, and collapsing it to three buttons would make the
hosted page test fewer scenarios than the API does. The form still needs an
"abandon" affordance (leave without paying) since a session can be closed
without a card being submitted at all — that path just navigates away
without submitting, no dedicated button/state needed since an abandoned
session simply expires or is explicitly expired via the existing
`POST /checkout/sessions/{id}/expire` API.

**Payment creation logic is extracted into a shared function inside
`providers::payments::acmepay`, called by both `routes::create_payment` and
the new `web::pages::acmepay_checkout` handler**, rather than duplicated.
Alternative considered: keep the dashboard page independent of
`providers::`, duplicating the resolve -> create -> advance-loop sequence.
Rejected: that sequence encodes real dialect behavior (which scenarios
short-circuit, how installments/capture_mode/risk fields are derived) —
duplicating it risks the two entry points silently drifting apart on the
next scenario added to `domain::scenario`. This is the first time a
dashboard page calls into a `providers::` module rather than only
`db::repo`; the extracted function itself still only calls `db::repo`
(the "nothing under `providers/` touches `sqlx::query*` directly" rule is
unaffected — no new SQL, just a new caller of the existing DB-touching
function).

**Redirect back to the merchant is a normal HTTP redirect to `success_url`/
`cancel_url` as stored, not Webpay's auto-submitting-form `POST`-back.**
Alternative considered: reuse Webpay's hidden-form/`POST` trick uniformly
for every hosted page. Rejected: that trick exists in Webpay's page
specifically because real Trancorp Webpay's API contract requires the
merchant's return endpoint to receive `token_ws` via `POST`. Acme Pay's own
dialect has no such requirement (spec §10.1); a merchant already holds the
checkout session id from their own `create` call and can `GET
/checkout/sessions/{id}` to confirm the outcome, so a plain redirect is
sufficient and matches how Stripe-shaped checkout normally returns control.

**Docs page renders the actual `acme.example.toml` file content (embedded
at compile time, same `rust_embed`/`include_str!` approach already used for
static assets) plus a hand-maintained key -> one-line-purpose table
underneath, rather than only prose.** Alternative considered: hand-write a
config-reference page with no direct tie to the file. Rejected: the
proposal's ask is specifically "see the Acme.toml example," and rendering
the real file (not a re-typed copy) guarantees the two can't drift on
formatting/comments/defaults; the added table is what turns the raw file
into "one-line purpose per key" without hand-duplicating the whole file a
second time. A cheap test asserting every key in the hand-maintained table
appears in the embedded file content catches the table going stale if a
key is renamed.

## Risks / Trade-offs

- **[Risk]** Extracting `create_payment`'s inner logic touches an
  already-tested, spec-locked handler (§9's "Definition of done" item 7
  requires an `insta` snapshot for Acme Pay). → **Mitigation**: extract by
  moving the body into a new function with the same inputs/outputs
  `create_payment` currently computes inline, and have `create_payment`
  call it — a pure refactor verified by the existing snapshot test
  continuing to pass unchanged, not a behavior change.
- **[Risk]** A hosted-page payment and an API-created payment now share one
  code path; a bug there affects both surfaces at once. → **Mitigation**:
  this is the intended trade-off (single source of truth for scenario
  resolution) and the existing `insta` snapshot coverage for
  `create_payment` continues to guard it; the new page adds its own
  snapshot/integration test for the session-linking and redirect behavior
  specifically.
- **[Trade-off]** The Docs page's hand-maintained description table can
  still drift in *wording* even with the presence test (it only checks the
  key exists in the file, not that the description stays accurate) —
  accepted, same maintenance burden as any hand-written doc comment
  elsewhere in this codebase.

## Migration Plan

No data migration. `checkout_sessions.payment_id` is already nullable and
already has the exact write path (`close_checkout_session`) this design
needs; no schema change. Purely additive: new routes, new page module, new
Docs page, one `hosted_url` construction change. Rollback is a revert — no
persisted state format changes.
