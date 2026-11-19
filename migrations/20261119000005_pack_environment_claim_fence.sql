-- A worker may retry its own expired installation. Its identity alone cannot
-- authorize a late renew/finish from the preceding claim.
ALTER TABLE pack_environment ADD COLUMN claim_id UUID;
UPDATE pack_environment SET claim_id=gen_random_uuid() WHERE claimed_by_worker IS NOT NULL;
ALTER TABLE pack_environment ADD CONSTRAINT pack_environment_claim_id_check
    CHECK (claim_id IS NULL OR claim_id <> '00000000-0000-0000-0000-000000000000'::UUID);
ALTER TABLE pack_environment ADD CONSTRAINT pack_environment_claim_present_check
    CHECK (claimed_by_worker IS NULL OR claim_id IS NOT NULL);
COMMENT ON COLUMN pack_environment.claim_id IS
    'Per-install claim fence; authority also requires matching worker, installing state and a live lease';
