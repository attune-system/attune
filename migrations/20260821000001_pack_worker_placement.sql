SET search_path TO attune, public;

ALTER TABLE pack
    ADD COLUMN worker_selector JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN worker_tolerations JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN worker_affinity JSONB NOT NULL DEFAULT '{}'::jsonb;

COMMENT ON COLUMN pack.worker_selector IS 'Mandatory worker labels inherited by every pack action, sensor, and pack test';
COMMENT ON COLUMN pack.worker_tolerations IS 'Mandatory worker tolerations inherited by every pack action, sensor, and pack test';
COMMENT ON COLUMN pack.worker_affinity IS 'Mandatory worker affinity inherited by every pack action, sensor, and pack test';
