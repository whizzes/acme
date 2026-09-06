-- M5 first slice (specs/006-Dialects.md item 1): short-lived bearer tokens
-- issued by an OAuth2 grant (Iberex's password grant today; any later
-- OAuth2 dialect reuses this same table). Not `webhook_deliveries`-style
-- persistent state — a row here is a session, cleaned up by expiry.
CREATE TABLE oauth_tokens (
  token         TEXT PRIMARY KEY,
  merchant_id   TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  provider_slug TEXT NOT NULL,
  expires_at    TEXT NOT NULL,
  created_at    TEXT NOT NULL
);
CREATE INDEX ix_oauth_tokens_expires ON oauth_tokens(expires_at);
