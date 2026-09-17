## Purpose

Lets a shopper complete an Acme Pay hosted checkout session by entering a
test card on a real page, resolving to an approved or declined payment the
same way the direct API does, and triggering the merchant's registered
webhook — replacing the `501` placeholder every checkout session currently
redirects to.

## ADDED Requirements

### Requirement: Hosted checkout page for an open session
An open Acme Pay checkout session's `hosted_url` SHALL resolve to a page
showing a card-entry form and a "simulated — do not enter real card details"
notice, instead of `501 Not Implemented`.

#### Scenario: Shopper opens an open session's hosted URL
- **WHEN** a shopper requests the `hosted_url` of a checkout session whose
  `status` is `open` and has not expired
- **THEN** the response is `200` and renders a card form (number, expiry,
  CVV, holder) together with the amount and currency being charged

#### Scenario: Shopper opens a closed, expired, or unknown session
- **WHEN** a shopper requests the `hosted_url` of a session that is
  `closed`, `expired`, or does not exist
- **THEN** the response shows a message that the checkout is no longer
  available and does not render a card form

### Requirement: Card submission resolves the same magic-value scenarios as the direct API
Submitting the hosted checkout form SHALL resolve the entered card through
the same magic-PAN/scenario rules `POST /acmepay/v1/payments` already uses,
creating a payment that is captured on approval or rejected on decline, and
SHALL close the checkout session against that outcome.

#### Scenario: Submitting an approved test card
- **WHEN** a shopper submits a PAN that resolves to an approve scenario
  (e.g. `4111 1111 1111 1111`) on an open session
- **THEN** a payment is created for the session's amount and currency,
  reaches `captured`, the session's `status` becomes `closed` and
  `payment_status` becomes `paid`, and the browser is redirected to the
  session's `success_url`

#### Scenario: Submitting a declined test card
- **WHEN** a shopper submits a PAN that resolves to a decline scenario
  (e.g. `4000 0000 0000 0002`) on an open session
- **THEN** a payment is created and reaches `rejected`, the session's
  `status` becomes `closed` and `payment_status` stays `unpaid`, and the
  browser is redirected to the session's `cancel_url`

#### Scenario: Session has no success_url or cancel_url configured
- **WHEN** a shopper completes payment (approved or declined) on a session
  created without a `success_url`/`cancel_url`
- **THEN** the page shows a confirmation message instead of redirecting,
  and the payment/session state changes described above still happen

#### Scenario: Resubmitting a session that is no longer open
- **WHEN** a shopper submits the card form for a session whose `status` is
  already `closed` or `expired`
- **THEN** no new payment is created, and the shopper sees a message that
  the checkout is no longer available

### Requirement: Checkout session outcomes trigger webhook delivery
A payment created through the hosted checkout page SHALL be dispatched to
the merchant's registered Acme Pay webhook endpoints through the existing
webhook dispatcher, identically to a payment created through the direct API.

#### Scenario: Merchant has a webhook endpoint registered for the outcome event
- **WHEN** a hosted checkout submission produces a payment event the
  merchant's webhook endpoint is subscribed to (e.g. `payment.captured`)
- **THEN** the webhook dispatcher delivers a signed request to that
  endpoint's URL, using the same signing scheme and retry behavior it
  already applies to API-created payments
