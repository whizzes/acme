## Why

Acme Pay's hosted-checkout flow is half-built: `POST /acmepay/v1/checkout/sessions`
already returns a `hosted_url`, but the browser lands on
`web::mod::hosted_checkout_placeholder` — a static `501`. A shopper can never
actually pay through the redirect flow, so the webhook dispatcher (already
built, already signing with `SigningScheme::Acme`, already retrying) never has
anything to fire for a checkout-session payment. Separately, the dashboard's
`/providers` page lists Acme Pay but only links out to Swagger — there is no
in-product answer to "how do I configure webhooks for this," even though every
knob (`webhooks_enabled`, `webhook_timeout_ms`, `webhook_max_attempts`) already
exists in `acme.toml`.

## What Changes

- New `GET /acmepay/c/{cs_id}` hosted checkout page: a card-entry form (styled
  like the existing Webpay checkout page, with the same "simulated — do not
  enter real card details" banner) that accepts the already-published magic
  test PANs (§9.1 — approve, decline variants, 3-D Secure, etc).
- New `POST /acmepay/c/{cs_id}`: runs the submitted PAN through the existing
  `domain::scenario` resolver (the same one `create_payment` already uses),
  creates/advances the underlying payment via the existing
  `domain::payment::apply` state machine, updates the checkout session's
  `status`/`payment_status`, then redirects the shopper's browser back to the
  merchant's `success_url`/`cancel_url` (Webpay's auto-submitting hidden-form
  pattern, reused).
- That payment transition is a normal `payments::advance` call, so the
  existing `webhooks::dispatcher` picks it up and delivers to whatever URL the
  merchant registered via `POST /acmepay/v1/webhook_endpoints` — no dispatcher
  or signing changes needed.
- `create_checkout_session` changes `hosted_url` from the generic
  `/checkout/{id}` (`501` stub) to the new `/acmepay/c/{cs_id}` path — the
  route name specs/x7-Additional-Dialects.md already plans for this so a
  future Chargeflow slice can share the same page component without a
  rename. The generic `/checkout/{token}` stub stays in place for every other
  provider until its own dialect lands.
- New "Docs" link on `/providers/acmepay` (alongside the existing "Open in
  Swagger UI" link) opening a new page that renders an annotated copy of
  `acme.example.toml`: every key, uncommented, with a one-line purpose —
  starting with the webhook-dispatch knobs, since those are what make this
  checkout flow's webhook delivery configurable. No new config keys — this
  documents what `acme.toml` already accepts.

## Capabilities

### New Capabilities

- `acmepay-hosted-checkout`: the hosted checkout page for Acme Pay — card
  form, magic-PAN scenario resolution, payment/session state update, merchant
  redirect, and the resulting webhook delivery via the existing dispatcher.
- `provider-docs`: the dashboard's per-provider "Docs" page, rendering the
  annotated `acme.toml` example.

### Modified Capabilities

(none — `openspec list --specs` returns no existing capabilities; this is the
first change to add any)

## Impact

- `crates/acme-server/src/providers/payments/acmepay/routes.rs`:
  `create_checkout_session`'s `hosted_url` construction.
- `crates/acme-server/src/web/mod.rs`: new route registration for
  `/acmepay/c/{cs_id}` (GET+POST), new route for the Docs page; existing
  `hosted_checkout_placeholder` stays for other providers.
- New `crates/acme-server/src/web/pages/acmepay_checkout.rs` (mirrors
  `web/pages/checkout.rs`'s Webpay page shape).
- New `crates/acme-server/src/web/pages/provider_docs.rs` (or an addition to
  `web/pages/providers.rs`).
- `crates/acme-server/src/web/pages/providers.rs`: add the "Docs" link to the
  detail page.
- `crates/acme-server/src/db/repo/payments.rs`: whatever checkout-session
  lookup/update helpers the new handler needs beyond what
  `get_checkout_session`/`set_checkout_session_payment_status` already
  provide (both exist today, used by Webpay's page).
- No `acme.toml`/`Config` struct changes.
- No changes to `webhooks::dispatcher`, `domain::webhook`, or
  `http::openapi` — all reused as-is.
