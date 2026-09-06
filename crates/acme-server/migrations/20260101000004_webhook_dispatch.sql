-- M4 (specs/005-Webhooks.md): auto-disable tracking for webhook_endpoints.
-- Reset to 0 on any successful delivery, incremented on failure/exhaustion;
-- the endpoint is auto-disabled once this reaches 20 (spec §12.4).
ALTER TABLE webhook_endpoints ADD COLUMN consecutive_failures INTEGER NOT NULL DEFAULT 0;
