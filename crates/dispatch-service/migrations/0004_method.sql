-- Add method column to tap_receipts.
--
-- The gateway now encodes the JSON-RPC method name as UTF-8 bytes at offset 20
-- in receipt metadata (after the 20-byte consumer address). We extract it at
-- receipt validation time and store it here for analytics queries.
--
-- NULL for receipts received before this change.

ALTER TABLE tap_receipts ADD COLUMN IF NOT EXISTS method TEXT;

CREATE INDEX IF NOT EXISTS tap_receipts_method ON tap_receipts (method);
