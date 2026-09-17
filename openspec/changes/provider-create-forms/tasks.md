## 1. Data layer

- [ ] 1.1 Add `merchant_id: MerchantId` to `db::repo::providers::CredentialRow` and its query in `demo_credential` (`crates/acme-server/src/db/repo/providers.rs`); verify `providers::tests` and any existing caller of `CredentialRow` still compile and pass.

## 2. Extract Acme Ship's one-shot create path

- [ ] 2.1 In `crates/acme-server/src/providers/shipping/acmeship/routes.rs`, extract `create_shipment`'s non-`rate_option_id` branch (validation, scenario resolution, pricing, row creation) into a `pub(crate)` fn taking plain inputs (origin/destination/packages/declared_value/service_code/order_reference/recipient_name/metadata/explicit_scenario, merchant_id), mirroring `acmepay::routes::ChargeInput`/`resolve_and_charge`'s shape.
- [ ] 2.2 Update `create_shipment` to call the extracted fn for its non-`rate_option_id` path; verify `cargo test -p acme-server` still passes the existing acmeship shipment-creation tests unchanged.

## 3. Dashboard mutations

- [ ] 3.1 In `crates/acme-server/src/web/mutations.rs`, add `create_payment_from_provider_page`: resolves the acmepay demo `merchant_id` via `providers::demo_credential`, builds a `ChargeInput` from form fields, calls `resolve_and_charge`, and returns a fragment showing the created payment's id/status or the validation/decline outcome.
- [ ] 3.2 Add `create_webhook_endpoint_from_provider_page`: resolves the acmepay demo `merchant_id`, creates a webhook endpoint via `db::repo::webhooks::create` the same way `acmepay::routes::create_webhook_endpoint` does, and returns a fragment showing the new endpoint's id, events, and generated secret.
- [ ] 3.3 Add `create_shipment_from_provider_page`: resolves the acmeship demo `merchant_id`, calls the fn extracted in 2.1, and returns a fragment showing the created shipment's tracking number/price or the coverage/validation outcome.
- [ ] 3.4 Register the three new routes in `crates/acme-server/src/web/mod.rs` (e.g. `POST /providers/acmepay/payments`, `POST /providers/acmepay/webhook_endpoints`, `POST /providers/acmeship/shipments`).

## 4. Provider detail page forms

- [ ] 4.1 In `crates/acme-server/src/web/pages/providers.rs`, replace the "Try it" section for `acmepay`'s `detail` with a payment-creation form (amount, currency, card number/exp/cvv/holder, reference) posting to the route from 3.4, HTMX-swapping in the result.
- [ ] 4.2 Add a webhook-endpoint registration form (url, enabled events, description) to the same `acmepay` detail page, posting to its route from 3.4.
- [ ] 4.3 Add a shipment-creation form (origin/destination postal code + country, one package's weight/dimensions, declared value, service code, recipient name, order reference) to `acmeship`'s `detail`, posting to its route from 3.4.
- [ ] 4.4 Keep the existing "Open in Swagger UI"/"Docs" links and the unchanged behavior for every other provider (including planned/unbuilt rows); verify by loading `/providers/webpay` and `/providers/iberex` and confirming no form appears.

## 5. Replace curl empty-states with links

- [ ] 5.1 In `crates/acme-server/src/web/pages/payments.rs`, replace the empty-state's `curl` argument with a link to `/providers/acmepay`.
- [ ] 5.2 In `crates/acme-server/src/web/pages/shipments.rs`, replace the empty-state's `curl` argument with a link to `/providers/acmeship`.
- [ ] 5.3 In `crates/acme-server/src/web/pages/webhooks.rs`, replace the empty-state's `curl` argument with a link to `/providers/acmepay`.
- [ ] 5.4 In `crates/acme-server/src/web/pages/overview.rs`, replace the empty-state's `curl` argument with a link to `/providers/acmepay`.
- [ ] 5.5 Update `components::empty_state`'s call sites accordingly; if no remaining caller passes a `curl` string, simplify its signature and verify `cargo build -p acme-server` has no unused-code warnings from the change.

## 6. Verification

- [ ] 6.1 Run `just run` (or `cargo run -p acme-server`), open `/providers/acmepay`, submit an approved test card, and confirm the created payment appears in `/payments` with status `captured`.
- [ ] 6.2 On the same page, register a webhook endpoint and confirm it appears in `/webhooks`.
- [ ] 6.3 Open `/providers/acmeship`, submit a shipment to a covered destination, and confirm it appears in `/shipments` with a tracking number.
- [ ] 6.4 Confirm `/payments`, `/shipments`, `/webhooks`, and the overview page's empty states (on a freshly reset sandbox via `/simulator` reset) link to the provider pages instead of showing `curl`.
- [ ] 6.5 Run `cargo test -p acme-server` and confirm the full suite passes.
