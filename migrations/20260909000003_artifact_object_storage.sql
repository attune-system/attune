CREATE TYPE artifact_body_state_enum AS ENUM ('pending', 'ready', 'deleting');

ALTER TABLE artifact_version
    ADD COLUMN body_state artifact_body_state_enum,
    ADD COLUMN object_key TEXT,
    ADD COLUMN provider_version TEXT,
    ADD COLUMN sha256 TEXT;

CREATE UNIQUE INDEX uq_artifact_version_object_key
    ON artifact_version(object_key)
    WHERE object_key IS NOT NULL;

ALTER TABLE artifact_version
    ADD CONSTRAINT ck_artifact_version_object_metadata CHECK (
        (body_state IS NULL AND object_key IS NULL AND provider_version IS NULL AND sha256 IS NULL)
        OR (body_state = 'pending' AND object_key IS NOT NULL AND provider_version IS NULL AND sha256 IS NULL AND size_bytes IS NULL)
        OR (body_state IN ('ready', 'deleting') AND object_key IS NOT NULL AND provider_version IS NOT NULL AND sha256 ~ '^[0-9a-f]{64}$' AND size_bytes IS NOT NULL AND size_bytes >= 0)
    );

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
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_artifact_body_immutability
    BEFORE UPDATE ON artifact_version
    FOR EACH ROW
    EXECUTE FUNCTION enforce_artifact_body_immutability();

-- Provider deletion must happen before metadata deletion. Keep the legacy
-- trigger for unmigrated/inline rows, but never cascade object metadata here.
CREATE OR REPLACE FUNCTION enforce_artifact_retention()
RETURNS TRIGGER AS $$
DECLARE
    v_policy artifact_retention_enum;
    v_limit INTEGER;
    v_count INTEGER;
BEGIN
    IF NEW.body_state IS NULL THEN
        SELECT retention_policy, retention_limit
        INTO v_policy, v_limit
        FROM artifact
        WHERE id = NEW.artifact;

        IF v_policy = 'versions' AND v_limit > 0 THEN
            SELECT COUNT(*) INTO v_count
            FROM artifact_version
            WHERE artifact = NEW.artifact AND body_state IS NULL;

            IF v_count > v_limit THEN
                DELETE FROM artifact_version
                WHERE id IN (
                    SELECT id
                    FROM artifact_version
                    WHERE artifact = NEW.artifact AND body_state IS NULL
                    ORDER BY version ASC
                    LIMIT (v_count - v_limit)
                );
            END IF;
        END IF;

        UPDATE artifact
        SET size_bytes = NEW.size_bytes,
            content_type = COALESCE(NEW.content_type, content_type)
        WHERE id = NEW.artifact;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

COMMENT ON COLUMN artifact_version.body_state IS 'Object-backed body lifecycle. NULL identifies an unmigrated filesystem or database row.';
COMMENT ON COLUMN artifact_version.object_key IS 'Immutable object-store locator reserved before upload.';
COMMENT ON COLUMN artifact_version.provider_version IS 'Opaque object-store generation, version ID, or ETag pinned for reads and deletion.';
COMMENT ON COLUMN artifact_version.sha256 IS 'Lowercase SHA-256 digest of the ready object body.';
