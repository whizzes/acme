# provider-try-it-forms Specification

## Purpose

Lets a merchant create a real payment, webhook endpoint, or shipment straight from the dashboard's provider detail pages, instead of having to copy a suggested `curl` command into a terminal.

## Requirements

### Requirement: Acme Pay provider page offers a payment-creation form
The `/providers/acmepay` page SHALL show a form that creates a real payment for the page's own demo merchant, resolving the same magic-value scenarios `POST /acmepay/v1/payments` uses.

#### Scenario: Submitting an approved test card
- **WHEN** a user submits the payment form on `/providers/acmepay` with an amount, currency, and a PAN that resolves to an approve scenario (e.g. `4111 1111 1111 1111`)
- **THEN** a payment is created for the demo merchant, reaches `captured`, and the page shows the created payment's id and status without a page navigation

#### Scenario: Submitting a declined test card
- **WHEN** a user submits the payment form with a PAN that resolves to a decline scenario (e.g. `4000 0000 0000 0002`)
- **THEN** a payment is created and reaches `rejected`, and the page shows the decline outcome instead of an error

#### Scenario: Submitting invalid input
- **WHEN** a user submits the payment form with a card number that fails Luhn validation, or omits a required field
- **THEN** no payment is created, and the page shows the validation problem without discarding the user's other entered field values

### Requirement: Acme Pay provider page offers a webhook-endpoint registration form
The `/providers/acmepay` page SHALL show a form that registers a real webhook endpoint for the page's own demo merchant, the same way `POST /acmepay/v1/webhook_endpoints` does.

#### Scenario: Registering an endpoint
- **WHEN** a user submits the webhook form on `/providers/acmepay` with a URL and one or more event types
- **THEN** a webhook endpoint is created for the demo merchant, and the page shows the new endpoint's id and registered events without a page navigation

#### Scenario: Registering with no event types selected
- **WHEN** a user submits the webhook form with no event types selected
- **THEN** the endpoint is created subscribed to every event type, matching `enabled_events: []` on the direct API

### Requirement: Acme Ship provider page offers a shipment-creation form
The `/providers/acmeship` page SHALL show a form that books a real shipment for the page's own demo merchant directly from origin, destination, package, and service details, without requiring a separate rate-quote step first.

#### Scenario: Booking a shipment with normal inputs
- **WHEN** a user submits the shipment form on `/providers/acmeship` with an origin, destination, one package's weight and dimensions, a declared value, and a service level
- **THEN** a shipment is created for the demo merchant with a tracking number and price, and the page shows it without a page navigation

#### Scenario: Booking to an uncovered destination
- **WHEN** a user submits the shipment form with a destination `acmeship` does not service
- **THEN** no shipment is created, and the page shows that the destination has no coverage

#### Scenario: Booking with invalid input
- **WHEN** a user submits the shipment form missing a required field (origin, destination, packages, or declared value)
- **THEN** no shipment is created, and the page shows the validation problem without discarding the user's other entered field values

### Requirement: Empty states link to the provider's create form instead of a curl command
Every dashboard page that previously suggested a `curl` command to create a first payment, shipment, or webhook endpoint SHALL instead link to the relevant provider detail page's form.

#### Scenario: Payments list has no payments yet
- **WHEN** a user opens `/payments` and no payments exist
- **THEN** the empty state links to `/providers/acmepay` instead of showing a `curl` command

#### Scenario: Shipments list has no shipments yet
- **WHEN** a user opens `/shipments` and no shipments exist
- **THEN** the empty state links to `/providers/acmeship` instead of showing a `curl` command

#### Scenario: Webhooks page has no endpoints yet
- **WHEN** a user opens `/webhooks` and no webhook endpoints exist
- **THEN** the empty state links to `/providers/acmepay` instead of showing a `curl` command

#### Scenario: Dashboard overview has no payments yet
- **WHEN** a user opens the dashboard overview and no payments exist
- **THEN** the empty state links to `/providers/acmepay` instead of showing a `curl` command
