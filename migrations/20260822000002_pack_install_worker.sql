ALTER TABLE pack_install
    ADD COLUMN assigned_worker_id BIGINT,
    ADD COLUMN candidate_access_token_hash TEXT;

-- Deployments can have durable messages from the old unscoped worker flow.
-- Fail those attempts rather than leave them running without an owner.
WITH interrupted AS (
    UPDATE pack_install
    SET status = 'failed',
        error_message = 'Pack test interrupted by worker assignment upgrade; retry the install',
        finished_at = COALESCE(finished_at, NOW()),
        updated_at = NOW()
    WHERE status = 'running'
    RETURNING pack_id
)
UPDATE pack
SET install_status = 'install_failed', updated = NOW()
WHERE id IN (SELECT pack_id FROM interrupted WHERE pack_id IS NOT NULL)
  AND install_status = 'pending';

CREATE INDEX idx_pack_install_assigned_worker
    ON pack_install(assigned_worker_id)
    WHERE assigned_worker_id IS NOT NULL;

COMMENT ON COLUMN pack_install.assigned_worker_id IS
    'Worker selected to execute this pack test; used to authorize candidate archive access';

COMMENT ON COLUMN pack_install.candidate_access_token_hash IS
    'SHA-256 hash of the attempt-scoped token required to download a staged candidate';
