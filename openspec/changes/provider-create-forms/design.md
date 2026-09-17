## Context

See proposal.md - Why. Relevant existing pieces:

- `acmepay::routes::resolve_and_charge` (`crates/acme-server/src/providers/payments/acmepay/routes.rs`) is already a `pub(crate)` fn shared by `POST /acmepay/v1/payments` and the hosted-checkout page (`web/pages/acmepay_checkout.rs`) — the precedent for calling dialect logic from a dashboard page without going through bearer auth.
- `acmeship::routes::create_shipment` (`crates/acme-server/src/providers/shipping/acmeship/routes.rs`) is a single `pub async fn` HTTP handler, not split into a reusable inner fn. Its non-`rate_option_id` branch (origin/destination/packages/declared_value/service_code given directly) is a complete one-shot booking path with no earlier `/rates` call needed.
- `web/mutations.rs` already holds every dashboard-only mutation (`advance_payment`, `refund_payment`, `advance_shipment`, `except_shipment`, sim controls) as plain axum handlers that call `db::repo::*` and domain logic directly, then re-render the affected page's fragment. New handlers follow this same shape.
- `db::repo::providers::demo_credential` returns `label`/`public_key`/`secret_key` but not `merchant_id` — every one of its two callers today (`/providers/{slug}` detail page) only displays the credential, never acts as that merchant.
- The dashboard is single-tenant: `db::bootstrap_demo_credentials` seeds exactly one demo merchant per provider, so "the merchant for this provider page" is unambiguous.

## Goals / Non-Goals

**Goals:**
- Reuse each dialect's own creation logic exactly, so a payment/shipment/webhook endpoint made from the dashboard is indistinguishable from one made through the real API.
- Keep the new mutations consistent with the existing `web/mutations.rs` pattern (direct domain call, HTMX fragment swap on success).

**Non-Goals:**
- Rate-shopping UI for Acme Ship (picking among priced service options before booking) — the form books directly against a chosen service code, mirroring `CreateShipmentRequest`'s one-shot path.
- Forms for `webpay` or `iberex` — out of scope per proposal.md - Impact.
- Any change to the public dialect APIs' request/response shapes, auth, or idempotency handling.

## Decisions

**Direct domain call, not a self-authenticated HTTP request.** The dashboard mutation resolves the demo `merchant_id` and calls `resolve_and_charge` / the extracted shipment-creation fn directly, the same way `acmepay_checkout.rs` already does for the hosted checkout page. Alternative considered: have the dashboard issue a real HTTP request to `/acmepay/v1/payments` using the demo `sk_test_` key. Rejected — it adds a network round-trip and duplicates auth/idempotency handling for what is a convenience feature, not a test of the public API surface.

**Extract Acme Ship's one-shot create path into a `pub(crate)` fn.** `create_shipment`'s non-`rate_option_id` branch (validation, scenario resolution, pricing, row creation) is pulled into a function taking plain inputs (mirroring `ChargeInput`/`resolve_and_charge`'s shape), callable from both the HTTP handler and the new dashboard mutation. Alternative considered: leave `create_shipment` as one handler and have the dashboard mutation construct the axum extractors (`Extension<AuthenticatedMerchant>`, `HeaderMap`, `Json`) to call it directly. Rejected — extractors aren't meant to be constructed by hand, and it would force the dashboard mutation to fabricate a fake `AuthenticatedMerchant` extension.

**Add `merchant_id` to `demo_credential`'s `CredentialRow`.** The query already selects from `api_credentials`, which has the column; this avoids a second query or an existing-caller-breaking signature split.

**Result rendering: inline fragment swap, no redirect.** Matches `payments::detail_fragment` / `shipments::detail_fragment`'s existing "swap the whole affected section" convention (spec §13.4 pattern 3, per the codebase's own comments) rather than introducing a redirect-based flow that would leave the provider page.

## Risks / Trade-offs

- [Duplicating "what a valid create-payment/create-shipment request looks like" between the HTTP DTO and the dashboard form] → The dashboard form posts field names that map onto the same `ChargeInput`/extracted-shipment-input struct the HTTP handler already builds from `CreatePaymentRequest`/`CreateShipmentRequest`, so the mapping lives in one place per provider, not duplicated validation logic.
- [Extracting Acme Ship's create path touches an existing, tested HTTP handler] → Existing route tests continue to exercise the extracted fn indirectly through the unchanged `POST /shipments` handler; no behavior change to the public endpoint.
- [Webhook endpoint secrets are generated and only ever shown once via the real API's response body] → The dashboard's inline result must display the generated `whsec_test_...` secret the same way the API response does, since the sandbox already shows secrets in full (spec §18) and this is the only chance to see it.

## Migration Plan

Additive only — no schema change beyond widening one existing query's projection, no removed routes, no change to existing HTTP behavior. Ships as a normal deploy; no rollback concerns beyond reverting the commit.
