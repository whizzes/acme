## Why

`/payments`, `/shipments`, `/webhooks`, and the dashboard overview all point an empty-state merchant at a raw `curl` command against the real dialect API to create their first record. The dashboard already has no way to create a payment, shipment, or webhook endpoint itself — every mutation it exposes today (`advance`, `refund`, `except`, sim controls) only acts on rows that already exist. A merchant exploring the sandbox in a browser has to leave it for a terminal.

## What Changes

- Add a "Try it" create-payment form to `/providers/acmepay`: amount, currency, card number/exp/cvv/holder, reference — submits directly through the same `resolve_and_charge` logic `POST /acmepay/v1/payments` and the hosted checkout page already share, using the page's own demo merchant.
- Add a "Register webhook endpoint" form to `/providers/acmepay`: url, enabled events, description — creates a real `webhook_endpoints` row the same way `POST /acmepay/v1/webhook_endpoints` does.
- Add a "Try it" create-shipment form to `/providers/acmeship`: origin/destination postal code + country, one package's weight/dimensions, declared value, service code, recipient name, order reference — submits through the one-shot (non-`rate_option_id`) branch of `acmeship`'s create-shipment logic, extracted into a reusable function the same way `acmepay::routes::resolve_and_charge` is, shared by the HTTP route and the new dashboard mutation.
- On success, each form's result swaps in inline (created payment/shipment summary or webhook endpoint row), matching the existing HTMX outerHTML pattern the detail pages already use for `advance`/`refund`.
- Replace the `curl` blocks in `payments.rs`, `shipments.rs`, `webhooks.rs`, and `overview.rs` empty-states with a link to the relevant provider page's new form.
- `db::repo::providers::demo_credential` gains the demo merchant's `merchant_id` (currently only returns label/public_key/secret_key) so the new mutations can resolve who they're creating the record for without a bearer token.
- Other providers (`webpay`, `iberex`) and the planned/unbuilt rows on `/providers` are unaffected — their detail pages keep only the existing "Open in Swagger UI"/"Docs" links.

## Capabilities

### New Capabilities
- `provider-try-it-forms`: provider detail pages for Acme Pay and Acme Ship gain live forms that create a real payment, webhook endpoint, or shipment from the dashboard, replacing the `curl` suggestions shown elsewhere.

### Modified Capabilities
(none — `acmepay-hosted-checkout` and `provider-docs` are unaffected; this change adds a new capability rather than changing an existing one's requirements)

## Impact

- `crates/acme-server/src/web/pages/providers.rs` — detail page gains the two acmepay forms and the one acmeship form.
- `crates/acme-server/src/web/mutations.rs` — new handlers: `create_payment_from_dashboard`, `create_webhook_endpoint_from_dashboard`, `create_shipment_from_dashboard`.
- `crates/acme-server/src/web/mod.rs` — new routes for the three form submissions.
- `crates/acme-server/src/providers/shipping/acmeship/routes.rs` — one-shot create-shipment path extracted into a `pub(crate)` fn reusable outside the HTTP handler.
- `crates/acme-server/src/db/repo/providers.rs` — `demo_credential`/`CredentialRow` gains `merchant_id`.
- `crates/acme-server/src/web/pages/{payments,shipments,webhooks,overview}.rs` — empty-state `curl` blocks replaced with a link.
- No changes to the public dialect APIs, their request/response shapes, or auth.
