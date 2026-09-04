-- M0 skeleton migration: proves the pool + migrate wiring end to end.
-- Full schema (merchants, providers, payments, shipments, webhooks, ...)
-- lands in M1 — see specs/000-Initial-Spec.md §7.
CREATE TABLE IF NOT EXISTS schema_meta (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    installed_at TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_meta (id, installed_at)
VALUES (1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
