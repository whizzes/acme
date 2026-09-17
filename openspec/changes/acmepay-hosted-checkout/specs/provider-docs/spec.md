## Purpose

Gives the dashboard's Providers section an in-product answer to "how do I
configure this provider, including its webhooks" by linking to a page that
documents every key `acme.toml` accepts, instead of sending the merchant only
to the Swagger API reference.

## ADDED Requirements

### Requirement: Provider detail page links to a Docs page
Each provider's detail page (`/providers/{slug}`) SHALL show a "Docs" link
alongside its existing "Open in Swagger UI" link.

#### Scenario: Viewing the Acme Pay provider detail page
- **WHEN** a user opens `/providers/acmepay`
- **THEN** the page shows a "Docs" link in addition to the existing "Open in
  Swagger UI" link

### Requirement: Docs page documents every acme.toml key
The Docs page SHALL render every configuration key available in
`acme.toml`, uncommented, each with a one-line description of what it
controls, sourced from the project's own example config rather than a
hand-maintained duplicate list.

#### Scenario: Opening the Docs page
- **WHEN** a user clicks "Docs" from a provider detail page
- **THEN** the page lists every key from the project's `acme.toml` example
  (bind address, database URL, seed/simulation settings, webhook dispatch
  settings, traffic capture settings, and so on), each with a one-line
  description

#### Scenario: Webhook configuration is explained as global, not per-provider
- **WHEN** a user reads the webhook-related entries on the Docs page
- **THEN** the page states that `webhooks_enabled`, `webhook_timeout_ms`,
  and `webhook_max_attempts` apply to every provider's webhook dispatch
  server-wide, and that individual webhook endpoint URLs/secrets are
  registered per merchant through the provider's API (e.g.
  `POST /acmepay/v1/webhook_endpoints`) or the dashboard, not through
  `acme.toml`
