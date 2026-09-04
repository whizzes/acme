-- Traffic inspector schema (spec §21.2), pulled into M1 by §21.14. Split
-- from the core schema migration because these tables — and
-- webhook_deliveries, slimmed per §21.1 — depend on http_exchanges.

CREATE TABLE http_bodies (
  id            TEXT PRIMARY KEY,
  content_type  TEXT,
  encoding      TEXT NOT NULL,
  size_bytes    INTEGER NOT NULL,
  stored_bytes  INTEGER NOT NULL,
  truncated     INTEGER NOT NULL DEFAULT 0,
  redactions    INTEGER NOT NULL DEFAULT 0,
  body          BLOB,
  refcount      INTEGER NOT NULL DEFAULT 1,
  created_at    TEXT NOT NULL
);

CREATE TABLE http_exchanges (
  id                 TEXT PRIMARY KEY,
  trace_id           TEXT NOT NULL,
  parent_id          TEXT REFERENCES http_exchanges(id),
  direction          TEXT NOT NULL CHECK (direction IN ('inbound','outbound')),
  channel            TEXT NOT NULL,
  started_at         TEXT NOT NULL,
  sim_at             TEXT NOT NULL,
  duration_ms        INTEGER,
  merchant_id        TEXT,
  provider_slug      TEXT,

  method             TEXT NOT NULL,
  url                TEXT NOT NULL,
  path               TEXT NOT NULL,
  query              TEXT,
  route_pattern      TEXT,
  http_version       TEXT NOT NULL DEFAULT 'HTTP/1.1',
  request_headers    TEXT NOT NULL,
  request_body_id    TEXT REFERENCES http_bodies(id),
  request_bytes      INTEGER NOT NULL DEFAULT 0,

  status_code        INTEGER,
  status_text        TEXT,
  response_headers   TEXT,
  response_body_id   TEXT REFERENCES http_bodies(id),
  response_bytes     INTEGER NOT NULL DEFAULT 0,
  transport_error    TEXT,

  outcome            TEXT NOT NULL,
  error_code         TEXT,
  fault_id           TEXT,
  idempotency_key    TEXT,
  idempotent_replay  INTEGER NOT NULL DEFAULT 0,
  auth_subject       TEXT,
  replay_of          TEXT REFERENCES http_exchanges(id),

  delivery_id        TEXT REFERENCES webhook_deliveries(id),
  event_id           TEXT REFERENCES events(id),
  attempt            INTEGER,
  signed_payload     TEXT,
  signature          TEXT,

  remote_ip          TEXT,
  user_agent         TEXT,

  search_key         TEXT
);

CREATE INDEX ix_ex_time      ON http_exchanges(started_at DESC);
CREATE INDEX ix_ex_trace     ON http_exchanges(trace_id, started_at);
CREATE INDEX ix_ex_provider  ON http_exchanges(provider_slug, started_at DESC);
CREATE INDEX ix_ex_outcome   ON http_exchanges(outcome, started_at DESC);
CREATE INDEX ix_ex_delivery  ON http_exchanges(delivery_id, attempt);
CREATE INDEX ix_ex_route     ON http_exchanges(route_pattern, started_at DESC);
CREATE INDEX ix_ex_search    ON http_exchanges(search_key);

CREATE TABLE exchange_resources (
  exchange_id   TEXT NOT NULL REFERENCES http_exchanges(id) ON DELETE CASCADE,
  resource_type TEXT NOT NULL,
  resource_id   TEXT NOT NULL,
  role          TEXT NOT NULL,
  PRIMARY KEY (exchange_id, resource_type, resource_id, role)
);
CREATE INDEX ix_exres_resource ON exchange_resources(resource_type, resource_id);

-- webhook_deliveries (spec §7.5), slimmed per §21.1: per-attempt HTTP detail
-- lives in http_exchanges (delivery_id, attempt), not here. Unused until M4.
CREATE TABLE webhook_deliveries (
  id            TEXT PRIMARY KEY,
  endpoint_id   TEXT NOT NULL REFERENCES webhook_endpoints(id) ON DELETE CASCADE,
  event_id      TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
  event_type    TEXT NOT NULL,
  trace_id      TEXT NOT NULL,
  payload_body_id TEXT REFERENCES http_bodies(id),
  attempt       INTEGER NOT NULL DEFAULT 0,
  max_attempts  INTEGER NOT NULL,
  status        TEXT NOT NULL,
  last_exchange_id TEXT REFERENCES http_exchanges(id),
  next_attempt_at TEXT,
  created_at    TEXT NOT NULL, delivered_at TEXT
);
CREATE INDEX ix_deliveries_due ON webhook_deliveries(next_attempt_at)
  WHERE status IN ('pending','failed');
CREATE INDEX ix_deliveries_trace ON webhook_deliveries(trace_id);
