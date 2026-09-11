ALTER TABLE pack_release
    ADD COLUMN legacy_snapshot_expires_at TIMESTAMPTZ;

ALTER TABLE artifact_version
    ADD COLUMN body_updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ADD COLUMN legacy_snapshot_expires_at TIMESTAMPTZ;

CREATE INDEX idx_artifact_version_body_maintenance
    ON artifact_version(body_state, body_updated, id)
    WHERE body_state IS NOT NULL;

CREATE OR REPLACE FUNCTION enforce_artifact_body_immutability()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.object_key IS NOT NULL AND NEW.object_key IS DISTINCT FROM OLD.object_key THEN
        RAISE EXCEPTION 'artifact object key is immutable';
    END IF;
    IF OLD.body_state IN ('ready', 'deleting') AND (
        NEW.provider_version IS DISTINCT FROM OLD.provider_version
        OR NEW.sha256 IS DISTINCT FROM OLD.sha256
        OR NEW.size_bytes IS DISTINCT FROM OLD.size_bytes
    ) THEN
        RAISE EXCEPTION 'ready artifact body metadata is immutable';
    END IF;
    IF OLD.body_state = 'ready' AND NEW.body_state NOT IN ('ready', 'deleting') THEN
        RAISE EXCEPTION 'ready artifact body can only transition to deleting';
    END IF;
    IF OLD.body_state = 'deleting' AND NEW.body_state <> 'deleting' THEN
        RAISE EXCEPTION 'deleting artifact body cannot transition';
    END IF;
    IF NEW.body_state IS DISTINCT FROM OLD.body_state THEN
        NEW.body_updated = NOW();
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

COMMENT ON COLUMN pack_release.legacy_snapshot_expires_at IS
    'Earliest time the migrated filesystem archive may be removed';
COMMENT ON COLUMN artifact_version.body_updated IS
    'Timestamp of the latest object-body lifecycle transition';
COMMENT ON COLUMN artifact_version.legacy_snapshot_expires_at IS
    'Earliest time the migrated filesystem body may be removed';
