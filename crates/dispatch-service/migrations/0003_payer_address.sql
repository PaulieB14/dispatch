-- Add payer_address to tap_receipts.
--
-- Previously the "payer" was implicitly the receipt signer (always the gateway).
-- Now the gateway embeds the consumer's address in receipt metadata, and we
-- store it here so the aggregator can produce per-consumer RAVs with the correct
-- RAV.payer — which is the address whose escrow is debited on-chain.
--
-- Existing rows are backfilled with signer_address (gateway = payer, old behaviour).

ALTER TABLE tap_receipts ADD COLUMN IF NOT EXISTS payer_address TEXT;
UPDATE tap_receipts SET payer_address = signer_address WHERE payer_address IS NULL;
ALTER TABLE tap_receipts ALTER COLUMN payer_address SET NOT NULL;

CREATE INDEX IF NOT EXISTS tap_receipts_payer ON tap_receipts (payer_address);
