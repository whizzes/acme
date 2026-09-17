## 1. Extract shared payment-creation logic

- [x] 1.1 In `providers::payments::acmepay`, extract `create_payment`'s body
      from scenario resolution through the `payment::apply`/
      `payments::advance` loop (routes.rs:81-227) into a new `pub(crate)`
      function taking the resolved inputs (amount, currency, payment
      method/card, capture mode, merchant id, now) and returning the
      created `PaymentId`/final `PaymentStatus`/decline reason. Have
      `create_payment` call it. Verify: existing `tests/scenarios.rs`
      (`create_payment_is_captured_immediately`,
      `declined_card_returns_402_and_still_persists_the_payment`) and any
      `insta` snapshot for Acme Pay still pass unchanged.

## 2. Hosted checkout page

- [x] 2.1 Add `crates/acme-server/src/web/pages/acmepay_checkout.rs`: `GET`
      handler loads the session via `db::repo::payments::get_checkout_session`,
      renders the card form (number/exp/cvv/holder + amount/currency) when
      `status == "open"` and not expired, otherwise renders an
      "unavailable" message with no form. Verify: manual `cargo run` +
      visiting the URL from a freshly created session, and the unavailable
      states via a session forced to `expired`/`closed`.
- [x] 2.2 Add the `POST` handler in the same file: on a non-open session,
      re-render the "unavailable" message and create nothing. On an open
      session, call the function from 1.1 with the submitted card and the
      session's stored amount/currency, then call
      `db::repo::payments::close_checkout_session` with the resulting
      `payment_id` and `paid`/`unpaid`. Verify: a new integration test (see
      3.1) covers both branches.
- [x] 2.3 After closing the session, redirect: to `success_url` when the
      payment reached `captured`, to `cancel_url` when it reached
      `rejected`, or to an in-page confirmation message when the
      corresponding URL is absent. Verify: same test as 2.2/3.1 asserts the
      redirect target (or confirmation body) per outcome.
- [x] 2.4 Style the page consistently with `web::pages::checkout` (Webpay):
      reuse or extend `static/checkout.css`, keep the persistent
      "simulated — do not enter real card details" banner. Verify: visual
      check via `cargo run`.
- [x] 2.5 Register routes in `web::mod`: `GET`/`POST`
      `/acmepay/c/{cs_id}`, pointing at the new handlers; leave the
      existing generic `/checkout/{token}` -> `hosted_checkout_placeholder`
      route untouched for other providers. Verify: route compiles and
      `just run` serves both paths without an "Overlapping method route"
      panic.
- [x] 2.6 Update `create_checkout_session` in
      `providers/payments/acmepay/routes.rs` to build `hosted_url` from
      `/acmepay/c/{id}` instead of the generic `/checkout/{id}`. Verify:
      `POST /acmepay/v1/payments` checkout-session creation test (new or
      existing) asserts the returned `hosted_url` contains `/acmepay/c/`.

## 3. Webhook delivery through the new flow

- [x] 3.1 Add an integration test (new `tests/checkout.rs` or an addition
      to `tests/webhooks.rs`, following the `spawn_mock_endpoint`/
      `register_endpoint` pattern already used there) that: creates a
      checkout session, registers a webhook endpoint, submits an approved
      test card to the new `POST /acmepay/c/{cs_id}`, and asserts the mock
      endpoint receives a signed `payment.captured` delivery. Verify: test
      passes under `cargo nextest run` (run locally, not by the agent, per
      `AGENTS.md`).
- [x] 3.2 Add the declined-card variant of the same test: asserts the
      resulting payment is `rejected` and (if the merchant is subscribed to
      it) a `payment.*` decline event is delivered, or that no delivery
      fires when the merchant isn't subscribed to that event. Verify: test
      passes under `cargo nextest run`.

## 4. Provider Docs page

- [x] 4.1 Embed `acme.example.toml`'s content at compile time (reuse the
      `rust_embed` approach `web::mod` already uses for `static/`, or a
      simple `include_str!`) and add
      `crates/acme-server/src/web/pages/provider_docs.rs` rendering it
      verbatim in a code block. Verify: `GET /providers/acmepay/docs`
      returns `200` and the response body contains the file's `bind =`
      line.
- [x] 4.2 Add a hand-maintained `const` table of `(key, one-line
      description)` pairs covering every key in `acme.example.toml`,
      rendered under the embedded file as a table; explicitly note that
      `webhooks_enabled`/`webhook_timeout_ms`/`webhook_max_attempts` are
      global (apply to every provider) and that per-endpoint URLs/secrets
      are registered via each provider's `POST /{slug}/v1/webhook_endpoints`
      API, not `acme.toml`. Verify: a unit test asserts every key in the
      const table appears as `key =` (commented or not) somewhere in the
      embedded `acme.example.toml` string, catching drift if a key is
      renamed.
- [x] 4.3 Register the route (`GET /providers/{slug}/docs`, or scope it to
      `/providers/acmepay/docs` if the table only makes sense per-provider
      for now — reusable verbatim for future providers regardless) in
      `web::mod`. Verify: route compiles and serves without panic.
- [x] 4.4 Add the "Docs" link next to "Open in Swagger UI" on
      `web::pages::providers::detail`. Verify: `GET /providers/acmepay`
      response body contains an `href` to the new route.

## 5. Wrap-up

- [x] 5.1 Corrected the one forward-looking stale claim
      (`specs/x7-Additional-Dialects.md` item 1 said Acme Pay's checkout
      path was "still a `501`" pending Chargeflow's own slice) to point at
      this change instead. `AGENTS.md`'s M0/M1-only "Status" section and
      the milestone specs' own point-in-time Caveats sections (e.g.
      `specs/x3-First-Vertical-Slice.md`) describe state as of *their own*
      milestone and are correct as historical record — well beyond this
      change's scope to bring current (the codebase is already many
      milestones ahead of what `AGENTS.md` claims, unrelated to this
      change).
- [ ] 5.2 Run the full local verification the agent cannot run itself and
      confirm green: `cargo fmt`, `cargo clippy`, `cargo nextest run`,
      `cargo build` (per `AGENTS.md` — run these locally, not by the
      agent).
