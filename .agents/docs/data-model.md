# Data model

Source: specs/000-Initial-Spec.md §7. **Implemented in M1** (specs/002-Core.md)
across two migrations: `crates/acme-server/migrations/20260101000002_core_schema.sql`
(everything below except `http_bodies`/`http_exchanges`/`exchange_resources`,
which need `webhook_deliveries` split out — see the traffic-inspector note
under §7.5) and `20260101000003_traffic_schema.sql` (those three plus
`webhook_deliveries`, slimmed per §21.1). M0's placeholder `schema_meta`
migration (`20260101000001_init.sql`) still runs first and is otherwise
unused. Kept here verbatim so later milestones can copy straight from it
instead of re-deriving it from the spec.

## Invariants (apply to every table)

- SQLite, **WAL mode**.
- Timestamps: RFC3339 **text**, UTC. `next_transition_at` "due" queries are
  plain string comparisons (`WHERE next_transition_at <= ?1`) — this only
  sorts correctly because the text is RFC3339 UTC throughout.
- Money: **integer minor units** (`_cents` columns) + an ISO-4217 currency
  code. Never floats.
- IDs: prefixed ULIDs (`pay_01JAV…`) — sortable, self-describing in logs.
- Connection pragmas (already wired in `db::connect`, §M0):
  ```sql
  PRAGMA journal_mode = WAL;
  PRAGMA synchronous = NORMAL;
  PRAGMA foreign_keys = ON;
  PRAGMA busy_timeout = 5000;
  PRAGMA temp_store = MEMORY;
  ```

## 7.1 Tenancy and catalog

```sql
CREATE TABLE merchants (
  id                TEXT PRIMARY KEY,           -- mrc_…
  name              TEXT NOT NULL,
  legal_name        TEXT,
  tax_id            TEXT,                       -- RUT / CUIT / CNPJ / NIF
  country           TEXT NOT NULL,              -- ISO-3166-1 alpha-2
  default_currency  TEXT NOT NULL,              -- ISO-4217
  timezone          TEXT NOT NULL DEFAULT 'UTC',
  created_at        TEXT NOT NULL
);

CREATE TABLE providers (
  slug          TEXT PRIMARY KEY,               -- 'chargeflow'
  kind          TEXT NOT NULL CHECK (kind IN ('payment','shipping')),
  display_name  TEXT NOT NULL,                  -- 'Chargeflow'
  dialect       TEXT NOT NULL,                  -- 'stripe-like'
  base_path     TEXT NOT NULL,                  -- '/chargeflow/v1'
  auth_scheme   TEXT NOT NULL,                  -- 'basic' | 'bearer' | 'header-pair' | 'oauth2'
  countries     TEXT NOT NULL,                  -- JSON array
  currencies    TEXT NOT NULL,                  -- JSON array
  capabilities  TEXT NOT NULL,                  -- JSON array: ['refunds','installments','pix',…]
  enabled       INTEGER NOT NULL DEFAULT 1,
  notes         TEXT
);

CREATE TABLE api_credentials (
  id            TEXT PRIMARY KEY,               -- key_…
  merchant_id   TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug TEXT NOT NULL REFERENCES providers(slug),
  label         TEXT,
  public_key    TEXT,                           -- pk_test_… where the dialect has one
  secret_key    TEXT NOT NULL,                  -- sk_test_… / Tbk secret / bearer token
  extra         TEXT,                           -- JSON: {"commerce_code":"597055555532","ccc":"12345"}
  active        INTEGER NOT NULL DEFAULT 1,
  created_at    TEXT NOT NULL,
  last_used_at  TEXT
);
CREATE UNIQUE INDEX ux_credentials_secret ON api_credentials(secret_key);
```

Every credential is scoped to exactly one provider. Presenting a `chargeflow`
key to `/pagorapido` is a 401 in the *Pago Rápido* error envelope.

## 7.2 Commerce

```sql
CREATE TABLE customers (
  id          TEXT PRIMARY KEY,                 -- cus_…
  merchant_id TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  email       TEXT NOT NULL,
  phone       TEXT,
  doc_type    TEXT,                             -- 'RUT','CPF','DNI','NIF'
  doc_number  TEXT,
  created_at  TEXT NOT NULL
);

CREATE TABLE addresses (
  id           TEXT PRIMARY KEY,                -- adr_…
  customer_id  TEXT REFERENCES customers(id) ON DELETE CASCADE,
  contact_name TEXT,
  phone        TEXT,
  line1        TEXT NOT NULL,
  line2        TEXT,
  district     TEXT,                            -- comuna / bairro / barrio
  city         TEXT NOT NULL,
  region       TEXT,                            -- state / provincia / comunidad
  postal_code  TEXT NOT NULL,
  country      TEXT NOT NULL,
  lat          REAL,
  lng          REAL,
  residential  INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE orders (
  id             TEXT PRIMARY KEY,              -- ord_…
  merchant_id    TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  customer_id    TEXT REFERENCES customers(id),
  reference      TEXT NOT NULL,                 -- merchant's own order number
  currency       TEXT NOT NULL,
  subtotal_cents INTEGER NOT NULL,
  shipping_cents INTEGER NOT NULL DEFAULT 0,
  tax_cents      INTEGER NOT NULL DEFAULT 0,
  total_cents    INTEGER NOT NULL,
  status         TEXT NOT NULL,                 -- draft|awaiting_payment|paid|fulfilling|shipped|delivered|cancelled|refunded
  ship_to_id     TEXT REFERENCES addresses(id),
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL
);
CREATE UNIQUE INDEX ux_orders_ref ON orders(merchant_id, reference);

CREATE TABLE order_items (
  id              TEXT PRIMARY KEY,
  order_id        TEXT NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
  sku             TEXT NOT NULL,
  name            TEXT NOT NULL,
  category        TEXT,
  quantity        INTEGER NOT NULL,
  unit_price_cents INTEGER NOT NULL,
  weight_grams    INTEGER NOT NULL,
  length_cm       INTEGER, width_cm INTEGER, height_cm INTEGER
);
```

## 7.3 Payments

```sql
CREATE TABLE payments (
  id                    TEXT PRIMARY KEY,       -- pay_…
  merchant_id           TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug         TEXT NOT NULL REFERENCES providers(slug),
  order_id              TEXT REFERENCES orders(id),
  session_id            TEXT,                   -- checkout_sessions.id if redirect flow
  external_ref          TEXT,                   -- buy_order / external_reference / client id
  amount_cents          INTEGER NOT NULL,
  currency              TEXT NOT NULL,
  amount_refunded_cents INTEGER NOT NULL DEFAULT 0,
  status                TEXT NOT NULL,          -- normalized, see domain.md §8.1
  status_reason         TEXT,                   -- normalized decline reason
  provider_status       TEXT,                   -- dialect string as returned to the caller
  provider_status_detail TEXT,
  capture_mode          TEXT NOT NULL DEFAULT 'automatic',  -- automatic|manual
  method_kind           TEXT NOT NULL,          -- card|pix|boleto|wallet|bnpl|bank_transfer|debit
  method_detail         TEXT,                   -- JSON: brand, last4, bank, issuer, installments
  installments          INTEGER NOT NULL DEFAULT 1,
  auth_code             TEXT,
  response_code         INTEGER,                -- 0 = approved, negative = declined (Webpay-style)
  risk_score            INTEGER,                -- 0..100
  risk_decision         TEXT,                   -- approve|review|reject
  three_ds              TEXT,                   -- none|frictionless|challenge|failed
  redirect_url          TEXT,
  return_url            TEXT,
  cancel_url            TEXT,
  scenario              TEXT,                   -- forced outcome key, see providers-and-scenarios.md §9
  metadata              TEXT,                   -- JSON
  next_transition_at    TEXT,                   -- sim-time the ticker acts on this row
  authorized_at         TEXT,
  captured_at           TEXT,
  expires_at            TEXT,
  created_at            TEXT NOT NULL,
  updated_at            TEXT NOT NULL
);
CREATE INDEX ix_payments_list ON payments(merchant_id, created_at DESC);
CREATE INDEX ix_payments_due  ON payments(next_transition_at) WHERE next_transition_at IS NOT NULL;

CREATE TABLE checkout_sessions (
  id              TEXT PRIMARY KEY,             -- cs_…
  merchant_id     TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug   TEXT NOT NULL REFERENCES providers(slug),
  payment_id      TEXT REFERENCES payments(id),
  token           TEXT NOT NULL,                -- token_ws / preference id / cs secret
  status          TEXT NOT NULL,                -- open|complete|expired|cancelled
  payment_status  TEXT NOT NULL,                -- unpaid|paid|no_payment_required
  amount_cents    INTEGER NOT NULL,
  currency        TEXT NOT NULL,
  line_items      TEXT,                         -- JSON
  customer_email  TEXT,
  success_url     TEXT, cancel_url TEXT,
  hosted_url      TEXT NOT NULL,                -- acme's own fake checkout page
  expires_at      TEXT NOT NULL,
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL
);
CREATE UNIQUE INDEX ux_sessions_token ON checkout_sessions(token);

CREATE TABLE payment_events (
  id          TEXT PRIMARY KEY,
  payment_id  TEXT NOT NULL REFERENCES payments(id) ON DELETE CASCADE,
  seq         INTEGER NOT NULL,
  type        TEXT NOT NULL,                    -- payment.authorized, payment.captured, …
  from_status TEXT, to_status TEXT,
  source      TEXT NOT NULL,                    -- api|ticker|dashboard|seed
  data        TEXT,                             -- JSON
  occurred_at TEXT NOT NULL                     -- sim time
);
CREATE UNIQUE INDEX ux_payment_events_seq ON payment_events(payment_id, seq);

CREATE TABLE refunds (
  id            TEXT PRIMARY KEY,               -- ref_…
  payment_id    TEXT NOT NULL REFERENCES payments(id) ON DELETE CASCADE,
  amount_cents  INTEGER NOT NULL,
  reason        TEXT,                           -- requested_by_customer|duplicate|fraudulent
  status        TEXT NOT NULL,                  -- pending|succeeded|failed|cancelled
  provider_ref  TEXT,
  nullified     INTEGER NOT NULL DEFAULT 0,     -- Webpay "nullify" vs true refund
  created_at    TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE disputes (
  id              TEXT PRIMARY KEY,             -- dsp_…
  payment_id      TEXT NOT NULL REFERENCES payments(id) ON DELETE CASCADE,
  amount_cents    INTEGER NOT NULL,
  reason          TEXT NOT NULL,                -- fraudulent|product_not_received|duplicate
  status          TEXT NOT NULL,                -- needs_response|under_review|won|lost
  evidence_due_at TEXT,
  created_at      TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE card_tokens (
  token       TEXT PRIMARY KEY,                 -- tok_… / card_…
  merchant_id TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  brand       TEXT NOT NULL,                    -- visa|mastercard|amex|elo|redcompra
  last4       TEXT NOT NULL,
  exp_month   INTEGER NOT NULL, exp_year INTEGER NOT NULL,
  holder      TEXT,
  scenario    TEXT,                             -- derived from the test PAN
  created_at  TEXT NOT NULL
);
```

## 7.4 Shipping

```sql
CREATE TABLE rate_quotes (
  id            TEXT PRIMARY KEY,               -- qte_…
  merchant_id   TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug TEXT NOT NULL REFERENCES providers(slug),
  order_id      TEXT REFERENCES orders(id),
  origin        TEXT NOT NULL,                  -- JSON address
  destination   TEXT NOT NULL,                  -- JSON address
  packages      TEXT NOT NULL,                  -- JSON array
  created_at    TEXT NOT NULL,
  expires_at    TEXT NOT NULL
);

CREATE TABLE rate_options (
  id                TEXT PRIMARY KEY,           -- rto_…
  quote_id          TEXT NOT NULL REFERENCES rate_quotes(id) ON DELETE CASCADE,
  carrier_code      TEXT NOT NULL,              -- 'iberex'
  service_code      TEXT NOT NULL,              -- 'express_10h'
  service_name      TEXT NOT NULL,
  amount_cents      INTEGER NOT NULL,           -- total, taxes included
  base_cents        INTEGER NOT NULL,
  surcharges        TEXT,                       -- JSON [{code,label,cents}]
  currency          TEXT NOT NULL,
  eta_min_days      INTEGER NOT NULL,
  eta_max_days      INTEGER NOT NULL,
  delivery_window   TEXT,                       -- JSON {start,end} for same-day services
  billable_weight_grams INTEGER NOT NULL,       -- max(actual, volumetric)
  insurance_cents   INTEGER NOT NULL DEFAULT 0,
  co2_grams         INTEGER
);

CREATE TABLE shipments (
  id                 TEXT PRIMARY KEY,          -- shp_…
  merchant_id        TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug      TEXT NOT NULL REFERENCES providers(slug),
  order_id           TEXT REFERENCES orders(id),
  rate_option_id     TEXT REFERENCES rate_options(id),
  tracking_number    TEXT NOT NULL,             -- provider-shaped, see spec §11
  external_ref       TEXT,                      -- expedition / localizador / label id
  carrier_code       TEXT NOT NULL,
  service_code       TEXT NOT NULL,
  origin             TEXT NOT NULL,             -- JSON address snapshot
  destination        TEXT NOT NULL,
  price_cents        INTEGER NOT NULL,
  currency           TEXT NOT NULL,
  declared_value_cents INTEGER NOT NULL DEFAULT 0,
  status             TEXT NOT NULL,             -- normalized, see domain.md §8.2
  status_reason      TEXT,
  provider_status_code TEXT,
  label_url          TEXT, label_format TEXT,   -- pdf|zpl|png
  eta_at             TEXT,
  promised_window    TEXT,                      -- JSON {start,end}
  delivery_attempts  INTEGER NOT NULL DEFAULT 0,
  proof_of_delivery  TEXT,                      -- JSON {signed_by, doc_number, photo_url, otp}
  return_tracking_number TEXT,
  scenario           TEXT,
  next_transition_at TEXT,
  created_at         TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX ux_shipments_tracking ON shipments(provider_slug, tracking_number);
CREATE INDEX ix_shipments_due ON shipments(next_transition_at) WHERE next_transition_at IS NOT NULL;

CREATE TABLE shipment_packages (
  id            TEXT PRIMARY KEY,
  shipment_id   TEXT NOT NULL REFERENCES shipments(id) ON DELETE CASCADE,
  idx           INTEGER NOT NULL,
  barcode       TEXT NOT NULL,
  weight_grams  INTEGER NOT NULL,
  length_cm INTEGER, width_cm INTEGER, height_cm INTEGER,
  contents      TEXT
);

CREATE TABLE shipment_events (
  id             TEXT PRIMARY KEY,
  shipment_id    TEXT NOT NULL REFERENCES shipments(id) ON DELETE CASCADE,
  seq            INTEGER NOT NULL,
  status         TEXT NOT NULL,                 -- normalized
  provider_code  TEXT NOT NULL,                 -- dialect code, e.g. 'EN_REPARTO'
  description    TEXT NOT NULL,                 -- localized to the provider's language
  facility       TEXT,                          -- 'Hub Madrid Sur'
  city TEXT, country TEXT, lat REAL, lng REAL,
  occurred_at    TEXT NOT NULL,
  created_at     TEXT NOT NULL
);
CREATE UNIQUE INDEX ux_shipment_events_seq ON shipment_events(shipment_id, seq);

CREATE TABLE pickups (
  id            TEXT PRIMARY KEY,               -- pck_…
  merchant_id   TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug TEXT NOT NULL REFERENCES providers(slug),
  address       TEXT NOT NULL,
  window_start  TEXT NOT NULL, window_end TEXT NOT NULL,
  shipment_ids  TEXT NOT NULL,                  -- JSON array
  status        TEXT NOT NULL,                  -- requested|confirmed|collected|cancelled|missed
  confirmation_code TEXT,
  created_at    TEXT NOT NULL
);

CREATE TABLE coverage (
  provider_slug  TEXT NOT NULL REFERENCES providers(slug),
  country        TEXT NOT NULL,
  postal_prefix  TEXT NOT NULL,                 -- '28', '5000', 'SW1'
  service_code   TEXT NOT NULL,
  supported      INTEGER NOT NULL DEFAULT 1,
  extra_days     INTEGER NOT NULL DEFAULT 0,
  surcharge_cents INTEGER NOT NULL DEFAULT 0,
  label          TEXT,                          -- 'Zona rural', 'Insular'
  PRIMARY KEY (provider_slug, country, postal_prefix, service_code)
);
```

## 7.5 Platform tables

```sql
CREATE TABLE events (
  id            TEXT PRIMARY KEY,               -- evt_…
  merchant_id   TEXT NOT NULL,
  provider_slug TEXT NOT NULL,
  type          TEXT NOT NULL,                  -- dialect-facing type string
  resource_type TEXT NOT NULL,                  -- payment|refund|shipment|dispute|pickup
  resource_id   TEXT NOT NULL,
  data          TEXT NOT NULL,                  -- JSON snapshot in dialect shape
  created_at    TEXT NOT NULL
);
CREATE INDEX ix_events_resource ON events(resource_type, resource_id);

CREATE TABLE webhook_endpoints (
  id            TEXT PRIMARY KEY,               -- whe_…
  merchant_id   TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug TEXT NOT NULL REFERENCES providers(slug),
  url           TEXT NOT NULL,
  secret        TEXT NOT NULL,                  -- whsec_…
  enabled_events TEXT NOT NULL,                 -- JSON array, ['*'] allowed
  active        INTEGER NOT NULL DEFAULT 1,
  description   TEXT,
  created_at    TEXT NOT NULL
);

-- A delivery is the logical intent to send one event to one endpoint.
-- The HTTP detail of each individual attempt lives in `http_exchanges` (spec §21).
CREATE TABLE webhook_deliveries (
  id            TEXT PRIMARY KEY,               -- whd_…
  endpoint_id   TEXT NOT NULL REFERENCES webhook_endpoints(id) ON DELETE CASCADE,
  event_id      TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
  event_type    TEXT NOT NULL,
  trace_id      TEXT NOT NULL,                  -- inherited from the causing request
  payload_body_id TEXT REFERENCES http_bodies(id),  -- the signed payload, deduped
  attempt       INTEGER NOT NULL DEFAULT 0,
  max_attempts  INTEGER NOT NULL,
  status        TEXT NOT NULL,                  -- pending|delivering|succeeded|failed|exhausted|cancelled
  last_exchange_id TEXT REFERENCES http_exchanges(id),
  next_attempt_at TEXT,
  created_at    TEXT NOT NULL, delivered_at TEXT
);
CREATE INDEX ix_deliveries_due ON webhook_deliveries(next_attempt_at)
  WHERE status IN ('pending','failed');
CREATE INDEX ix_deliveries_trace ON webhook_deliveries(trace_id);

-- Inbound API calls and outbound webhook attempts share one table: see spec §21.
-- CREATE TABLE http_exchanges (…);
-- CREATE TABLE http_bodies (…);
-- CREATE TABLE exchange_resources (…);

CREATE TABLE idempotency_keys (
  merchant_id     TEXT NOT NULL,
  endpoint        TEXT NOT NULL,
  key             TEXT NOT NULL,
  request_hash    TEXT NOT NULL,                -- sha256 of canonicalized body
  response_status INTEGER,
  response_body   TEXT,
  state           TEXT NOT NULL,                -- in_flight|complete
  created_at      TEXT NOT NULL,
  expires_at      TEXT NOT NULL,
  PRIMARY KEY (merchant_id, endpoint, key)
);

CREATE TABLE sim_settings (
  id                integer PRIMARY KEY CHECK (id = 1),
  clock_epoch       TEXT NOT NULL,
  clock_started_at  TEXT NOT NULL,              -- wall clock when epoch was set
  multiplier        REAL NOT NULL DEFAULT 60.0,
  paused            INTEGER NOT NULL DEFAULT 0,
  latency_ms        INTEGER NOT NULL DEFAULT 0,
  failure_rate      REAL NOT NULL DEFAULT 0.0,
  webhook_failure_rate REAL NOT NULL DEFAULT 0.0,
  updated_at        TEXT NOT NULL
);

CREATE TABLE sim_faults (
  id            TEXT PRIMARY KEY,               -- flt_…
  provider_slug TEXT,                           -- NULL = all
  method        TEXT,                           -- NULL = all
  path_glob     TEXT NOT NULL,                  -- '/chargeflow/v1/checkout/*'
  mode          TEXT NOT NULL,                  -- error|latency|timeout|malformed|rate_limit
  http_status   INTEGER, error_code TEXT,
  latency_ms    INTEGER,
  probability   REAL NOT NULL DEFAULT 1.0,
  remaining     INTEGER,                        -- fire N times then auto-disable
  active        INTEGER NOT NULL DEFAULT 1,
  note          TEXT,
  created_at    TEXT NOT NULL, expires_at TEXT
);
```

`http_exchanges`/`http_bodies`/`exchange_resources` (spec §21.2) are
implemented in `20260101000003_traffic_schema.sql`, not reproduced here —
read the spec or that migration file directly. Populated by
`capture::recorder` (inbound only in M1; outbound/webhook capture is M4).
