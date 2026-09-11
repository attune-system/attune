ALTER TABLE pack_release
    ADD COLUMN inactive_since TIMESTAMPTZ;

UPDATE pack_release r
SET inactive_since = COALESCE(p.updated, r.created)
FROM pack p
WHERE p.id = r.pack
  AND p.active_release IS DISTINCT FROM r.id;

CREATE INDEX idx_pack_release_inactive_retention
    ON pack_release(pack, inactive_since DESC, id DESC)
    WHERE inactive_since IS NOT NULL;
CREATE INDEX idx_execution_pack_release_pin
    ON execution(pack_release) WHERE pack_release IS NOT NULL;
CREATE INDEX idx_enforcement_pack_release_pin
    ON enforcement(pack_release) WHERE pack_release IS NOT NULL;
CREATE INDEX idx_work_queue_item_pack_release_pin
    ON work_queue_item(pack_release) WHERE pack_release IS NOT NULL;
CREATE INDEX idx_sensor_workload_pack_release_pin
    ON sensor_workload(pack_release) WHERE pack_release IS NOT NULL;

CREATE TABLE object_maintenance_ledger (
    id BIGSERIAL PRIMARY KEY,
    object_key TEXT NOT NULL UNIQUE,
    provider_version TEXT,
    object_kind TEXT NOT NULL CHECK (object_kind IN ('pack', 'artifact', 'log')),
    size_bytes BIGINT CHECK (size_bytes IS NULL OR size_bytes >= 0),
    state TEXT NOT NULL DEFAULT 'uploading'
        CHECK (state IN ('uploading', 'ready', 'deletion_pending', 'deleting')),
    eligible_at TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK ((state = 'uploading' AND provider_version IS NULL)
        OR (state <> 'uploading' AND provider_version IS NOT NULL))
);

CREATE INDEX idx_object_maintenance_collect
    ON object_maintenance_ledger(state, eligible_at, updated, id);

INSERT INTO object_maintenance_ledger
    (object_key, provider_version, object_kind, size_bytes, state)
SELECT object_key, provider_version, 'pack', archive_size, 'ready'
FROM pack_release
WHERE object_key IS NOT NULL
ON CONFLICT (object_key) DO NOTHING;

INSERT INTO object_maintenance_ledger
    (object_key, provider_version, object_kind, size_bytes, state)
SELECT av.object_key, av.provider_version, 'artifact', av.size_bytes, 'ready'
FROM artifact_version av
WHERE av.object_key IS NOT NULL AND av.provider_version IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM log_stream ls WHERE ls.artifact_version = av.id)
ON CONFLICT (object_key) DO NOTHING;

INSERT INTO object_maintenance_ledger
    (object_key, provider_version, object_kind, size_bytes, state)
SELECT object_key, provider_version, 'log', size_bytes, 'ready'
FROM log_segment
ON CONFLICT (object_key) DO NOTHING;

CREATE OR REPLACE FUNCTION enqueue_deleted_object()
RETURNS TRIGGER AS $$
DECLARE
    v_size BIGINT;
BEGIN
    IF TG_TABLE_NAME = 'artifact_version'
       AND EXISTS (SELECT 1 FROM log_stream WHERE artifact_version = OLD.id) THEN
        RETURN OLD;
    END IF;
    IF OLD.object_key IS NOT NULL AND OLD.provider_version IS NOT NULL THEN
        IF TG_TABLE_NAME = 'pack_release' THEN
            v_size := OLD.archive_size;
        ELSE
            v_size := OLD.size_bytes;
        END IF;
        INSERT INTO object_maintenance_ledger
            (object_key, provider_version, object_kind, size_bytes, state, eligible_at, updated)
        VALUES (
            OLD.object_key,
            OLD.provider_version,
            TG_ARGV[0],
            v_size,
            'deletion_pending',
            NOW(),
            NOW()
        )
        ON CONFLICT (object_key) DO UPDATE
        SET state = 'deletion_pending', eligible_at = NOW(), updated = NOW(), last_error = NULL
        WHERE object_maintenance_ledger.provider_version = EXCLUDED.provider_version;
    END IF;
    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_pack_release_enqueue_object
    BEFORE DELETE ON pack_release FOR EACH ROW
    EXECUTE FUNCTION enqueue_deleted_object('pack');
CREATE TRIGGER trg_artifact_version_enqueue_object
    BEFORE DELETE ON artifact_version FOR EACH ROW
    EXECUTE FUNCTION enqueue_deleted_object('artifact');
CREATE TRIGGER trg_log_segment_enqueue_object
    BEFORE DELETE ON log_segment FOR EACH ROW
    EXECUTE FUNCTION enqueue_deleted_object('log');

CREATE OR REPLACE FUNCTION enforce_artifact_retention()
RETURNS TRIGGER AS $$
DECLARE
    v_policy artifact_retention_enum;
    v_limit INTEGER;
BEGIN
    IF NEW.body_state = 'pending' OR NEW.body_state = 'deleting' THEN
        RETURN NEW;
    END IF;

    SELECT retention_policy, retention_limit INTO v_policy, v_limit
    FROM artifact WHERE id = NEW.artifact;

    IF v_policy = 'versions' AND v_limit > 0 THEN
        DELETE FROM artifact_version
        WHERE id IN (
            SELECT stale.id FROM artifact_version stale
            WHERE stale.artifact = NEW.artifact
              AND (stale.body_state IS NULL OR stale.body_state = 'ready')
              AND stale.id NOT IN (
                  SELECT kept.id FROM artifact_version kept
                  WHERE kept.artifact = NEW.artifact
                    AND (kept.body_state IS NULL OR kept.body_state = 'ready')
                  ORDER BY kept.version DESC LIMIT v_limit
              )
            ORDER BY stale.version ASC LIMIT 100
        );
    END IF;

    UPDATE artifact
    SET size_bytes = NEW.size_bytes,
        content_type = COALESCE(NEW.content_type, content_type)
    WHERE id = NEW.artifact;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_enforce_artifact_retention_ready
    AFTER UPDATE OF body_state ON artifact_version
    FOR EACH ROW
    WHEN (OLD.body_state = 'pending' AND NEW.body_state = 'ready')
    EXECUTE FUNCTION enforce_artifact_retention();

COMMENT ON COLUMN pack_release.inactive_since IS
    'Time this release stopped being active; NULL means active or never activated';
COMMENT ON TABLE object_maintenance_ledger IS
    'Durable upload and exact-version object deletion work; collectors never list buckets';
