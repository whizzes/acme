-- Full core schema (spec §7.1-7.5, minus webhook_deliveries/exchange_resources
-- which depend on http_exchanges — see 20260101000003_traffic_schema.sql).

-- §7.1 Tenancy and catalog

CREATE TABLE merchants (
  id                TEXT PRIMARY KEY,
  name              TEXT NOT NULL,
  legal_name        TEXT,
  tax_id            TEXT,
  country           TEXT NOT NULL,
  default_currency  TEXT NOT NULL,
  timezone          TEXT NOT NULL DEFAULT 'UTC',
  created_at        TEXT NOT NULL
);

CREATE TABLE providers (
  slug          TEXT PRIMARY KEY,
  kind          TEXT NOT NULL CHECK (kind IN ('payment','shipping')),
  display_name  TEXT NOT NULL,
  dialect       TEXT NOT NULL,
  base_path     TEXT NOT NULL,
  auth_scheme   TEXT NOT NULL,
  countries     TEXT NOT NULL,
  currencies    TEXT NOT NULL,
  capabilities  TEXT NOT NULL,
  enabled       INTEGER NOT NULL DEFAULT 1,
  notes         TEXT
);

CREATE TABLE api_credentials (
  id            TEXT PRIMARY KEY,
  merchant_id   TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug TEXT NOT NULL REFERENCES providers(slug),
  label         TEXT,
  public_key    TEXT,
  secret_key    TEXT NOT NULL,
  extra         TEXT,
  active        INTEGER NOT NULL DEFAULT 1,
  created_at    TEXT NOT NULL,
  last_used_at  TEXT
);
CREATE UNIQUE INDEX ux_credentials_secret ON api_credentials(secret_key);

-- §7.2 Commerce

CREATE TABLE customers (
  id          TEXT PRIMARY KEY,
  merchant_id TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  email       TEXT NOT NULL,
  phone       TEXT,
  doc_type    TEXT,
  doc_number  TEXT,
  created_at  TEXT NOT NULL
);

CREATE TABLE addresses (
  id           TEXT PRIMARY KEY,
  customer_id  TEXT REFERENCES customers(id) ON DELETE CASCADE,
  contact_name TEXT,
  phone        TEXT,
  line1        TEXT NOT NULL,
  line2        TEXT,
  district     TEXT,
  city         TEXT NOT NULL,
  region       TEXT,
  postal_code  TEXT NOT NULL,
  country      TEXT NOT NULL,
  lat          REAL,
  lng          REAL,
  residential  INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE orders (
  id             TEXT PRIMARY KEY,
  merchant_id    TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  customer_id    TEXT REFERENCES customers(id),
  reference      TEXT NOT NULL,
  currency       TEXT NOT NULL,
  subtotal_cents INTEGER NOT NULL,
  shipping_cents INTEGER NOT NULL DEFAULT 0,
  tax_cents      INTEGER NOT NULL DEFAULT 0,
  total_cents    INTEGER NOT NULL,
  status         TEXT NOT NULL,
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

-- §7.3 Payments

CREATE TABLE payments (
  id                    TEXT PRIMARY KEY,
  merchant_id           TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug         TEXT NOT NULL REFERENCES providers(slug),
  order_id              TEXT REFERENCES orders(id),
  session_id            TEXT,
  external_ref          TEXT,
  amount_cents          INTEGER NOT NULL,
  currency              TEXT NOT NULL,
  amount_refunded_cents INTEGER NOT NULL DEFAULT 0,
  status                TEXT NOT NULL,
  status_reason         TEXT,
  provider_status       TEXT,
  provider_status_detail TEXT,
  capture_mode          TEXT NOT NULL DEFAULT 'automatic',
  method_kind           TEXT NOT NULL,
  method_detail         TEXT,
  installments          INTEGER NOT NULL DEFAULT 1,
  auth_code             TEXT,
  response_code         INTEGER,
  risk_score            INTEGER,
  risk_decision         TEXT,
  three_ds              TEXT,
  redirect_url          TEXT,
  return_url            TEXT,
  cancel_url            TEXT,
  scenario              TEXT,
  metadata              TEXT,
  next_transition_at    TEXT,
  authorized_at         TEXT,
  captured_at           TEXT,
  expires_at            TEXT,
  created_at            TEXT NOT NULL,
  updated_at            TEXT NOT NULL
);
CREATE INDEX ix_payments_list ON payments(merchant_id, created_at DESC);
CREATE INDEX ix_payments_due  ON payments(next_transition_at) WHERE next_transition_at IS NOT NULL;

CREATE TABLE checkout_sessions (
  id              TEXT PRIMARY KEY,
  merchant_id     TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug   TEXT NOT NULL REFERENCES providers(slug),
  payment_id      TEXT REFERENCES payments(id),
  token           TEXT NOT NULL,
  status          TEXT NOT NULL,
  payment_status  TEXT NOT NULL,
  amount_cents    INTEGER NOT NULL,
  currency        TEXT NOT NULL,
  line_items      TEXT,
  customer_email  TEXT,
  success_url     TEXT, cancel_url TEXT,
  hosted_url      TEXT NOT NULL,
  expires_at      TEXT NOT NULL,
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL
);
CREATE UNIQUE INDEX ux_sessions_token ON checkout_sessions(token);

CREATE TABLE payment_events (
  id          TEXT PRIMARY KEY,
  payment_id  TEXT NOT NULL REFERENCES payments(id) ON DELETE CASCADE,
  seq         INTEGER NOT NULL,
  type        TEXT NOT NULL,
  from_status TEXT, to_status TEXT,
  source      TEXT NOT NULL,
  data        TEXT,
  occurred_at TEXT NOT NULL
);
CREATE UNIQUE INDEX ux_payment_events_seq ON payment_events(payment_id, seq);

CREATE TABLE refunds (
  id            TEXT PRIMARY KEY,
  payment_id    TEXT NOT NULL REFERENCES payments(id) ON DELETE CASCADE,
  amount_cents  INTEGER NOT NULL,
  reason        TEXT,
  status        TEXT NOT NULL,
  provider_ref  TEXT,
  nullified     INTEGER NOT NULL DEFAULT 0,
  created_at    TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE disputes (
  id              TEXT PRIMARY KEY,
  payment_id      TEXT NOT NULL REFERENCES payments(id) ON DELETE CASCADE,
  amount_cents    INTEGER NOT NULL,
  reason          TEXT NOT NULL,
  status          TEXT NOT NULL,
  evidence_due_at TEXT,
  created_at      TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE card_tokens (
  token       TEXT PRIMARY KEY,
  merchant_id TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  brand       TEXT NOT NULL,
  last4       TEXT NOT NULL,
  exp_month   INTEGER NOT NULL, exp_year INTEGER NOT NULL,
  holder      TEXT,
  scenario    TEXT,
  created_at  TEXT NOT NULL
);

-- §7.4 Shipping

CREATE TABLE rate_quotes (
  id            TEXT PRIMARY KEY,
  merchant_id   TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug TEXT NOT NULL REFERENCES providers(slug),
  order_id      TEXT REFERENCES orders(id),
  origin        TEXT NOT NULL,
  destination   TEXT NOT NULL,
  packages      TEXT NOT NULL,
  created_at    TEXT NOT NULL,
  expires_at    TEXT NOT NULL
);

CREATE TABLE rate_options (
  id                TEXT PRIMARY KEY,
  quote_id          TEXT NOT NULL REFERENCES rate_quotes(id) ON DELETE CASCADE,
  carrier_code      TEXT NOT NULL,
  service_code      TEXT NOT NULL,
  service_name      TEXT NOT NULL,
  amount_cents      INTEGER NOT NULL,
  base_cents        INTEGER NOT NULL,
  surcharges        TEXT,
  currency          TEXT NOT NULL,
  eta_min_days      INTEGER NOT NULL,
  eta_max_days      INTEGER NOT NULL,
  delivery_window   TEXT,
  billable_weight_grams INTEGER NOT NULL,
  insurance_cents   INTEGER NOT NULL DEFAULT 0,
  co2_grams         INTEGER
);

CREATE TABLE shipments (
  id                 TEXT PRIMARY KEY,
  merchant_id        TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug      TEXT NOT NULL REFERENCES providers(slug),
  order_id           TEXT REFERENCES orders(id),
  rate_option_id     TEXT REFERENCES rate_options(id),
  tracking_number    TEXT NOT NULL,
  external_ref       TEXT,
  carrier_code       TEXT NOT NULL,
  service_code       TEXT NOT NULL,
  origin             TEXT NOT NULL,
  destination        TEXT NOT NULL,
  price_cents        INTEGER NOT NULL,
  currency           TEXT NOT NULL,
  declared_value_cents INTEGER NOT NULL DEFAULT 0,
  status             TEXT NOT NULL,
  status_reason      TEXT,
  provider_status_code TEXT,
  label_url          TEXT, label_format TEXT,
  eta_at             TEXT,
  promised_window    TEXT,
  delivery_attempts  INTEGER NOT NULL DEFAULT 0,
  proof_of_delivery  TEXT,
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
  status         TEXT NOT NULL,
  provider_code  TEXT NOT NULL,
  description    TEXT NOT NULL,
  facility       TEXT,
  city TEXT, country TEXT, lat REAL, lng REAL,
  occurred_at    TEXT NOT NULL,
  created_at     TEXT NOT NULL
);
CREATE UNIQUE INDEX ux_shipment_events_seq ON shipment_events(shipment_id, seq);

CREATE TABLE pickups (
  id            TEXT PRIMARY KEY,
  merchant_id   TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug TEXT NOT NULL REFERENCES providers(slug),
  address       TEXT NOT NULL,
  window_start  TEXT NOT NULL, window_end TEXT NOT NULL,
  shipment_ids  TEXT NOT NULL,
  status        TEXT NOT NULL,
  confirmation_code TEXT,
  created_at    TEXT NOT NULL
);

CREATE TABLE coverage (
  provider_slug  TEXT NOT NULL REFERENCES providers(slug),
  country        TEXT NOT NULL,
  postal_prefix  TEXT NOT NULL,
  service_code   TEXT NOT NULL,
  supported      INTEGER NOT NULL DEFAULT 1,
  extra_days     INTEGER NOT NULL DEFAULT 0,
  surcharge_cents INTEGER NOT NULL DEFAULT 0,
  label          TEXT,
  PRIMARY KEY (provider_slug, country, postal_prefix, service_code)
);

-- §7.5 Platform tables (webhook_deliveries and exchange_resources follow in
-- the next migration, once http_exchanges exists)

CREATE TABLE events (
  id            TEXT PRIMARY KEY,
  merchant_id   TEXT NOT NULL,
  provider_slug TEXT NOT NULL,
  type          TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id   TEXT NOT NULL,
  data          TEXT NOT NULL,
  created_at    TEXT NOT NULL
);
CREATE INDEX ix_events_resource ON events(resource_type, resource_id);

CREATE TABLE webhook_endpoints (
  id            TEXT PRIMARY KEY,
  merchant_id   TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug TEXT NOT NULL REFERENCES providers(slug),
  url           TEXT NOT NULL,
  secret        TEXT NOT NULL,
  enabled_events TEXT NOT NULL,
  active        INTEGER NOT NULL DEFAULT 1,
  description   TEXT,
  created_at    TEXT NOT NULL
);

CREATE TABLE idempotency_keys (
  merchant_id     TEXT NOT NULL,
  endpoint        TEXT NOT NULL,
  key             TEXT NOT NULL,
  request_hash    TEXT NOT NULL,
  response_status INTEGER,
  response_body   TEXT,
  state           TEXT NOT NULL,
  created_at      TEXT NOT NULL,
  expires_at      TEXT NOT NULL,
  PRIMARY KEY (merchant_id, endpoint, key)
);

CREATE TABLE sim_settings (
  id                INTEGER PRIMARY KEY CHECK (id = 1),
  clock_epoch       TEXT NOT NULL,
  clock_started_at  TEXT NOT NULL,
  multiplier        REAL NOT NULL DEFAULT 60.0,
  paused            INTEGER NOT NULL DEFAULT 0,
  latency_ms        INTEGER NOT NULL DEFAULT 0,
  failure_rate      REAL NOT NULL DEFAULT 0.0,
  webhook_failure_rate REAL NOT NULL DEFAULT 0.0,
  updated_at        TEXT NOT NULL
);

CREATE TABLE sim_faults (
  id            TEXT PRIMARY KEY,
  provider_slug TEXT,
  method        TEXT,
  path_glob     TEXT NOT NULL,
  mode          TEXT NOT NULL,
  http_status   INTEGER, error_code TEXT,
  latency_ms    INTEGER,
  probability   REAL NOT NULL DEFAULT 1.0,
  remaining     INTEGER,
  active        INTEGER NOT NULL DEFAULT 1,
  note          TEXT,
  created_at    TEXT NOT NULL, expires_at TEXT
);
