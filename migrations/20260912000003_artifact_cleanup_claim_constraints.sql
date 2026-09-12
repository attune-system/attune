ALTER TABLE artifact_version
    DROP CONSTRAINT ck_artifact_version_object_metadata,
    ADD CONSTRAINT ck_artifact_version_object_metadata CHECK (
        (body_state IS NULL AND object_key IS NULL AND provider_version IS NULL AND sha256 IS NULL)
        OR (
            body_state IN ('pending', 'cleanup_claimed')
            AND provider_version IS NULL
            AND sha256 IS NULL
            AND size_bytes IS NULL
        )
        OR (
            body_state IN ('ready', 'deleting')
            AND size_bytes IS NOT NULL
            AND size_bytes >= 0
            AND sha256 ~ '^[0-9a-f]{64}$'
            AND (
                (object_key IS NOT NULL AND provider_version IS NOT NULL)
                OR (object_key IS NULL AND provider_version IS NULL AND file_path IS NOT NULL)
            )
        )
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
    IF OLD.body_state IN ('deleting', 'cleanup_claimed') AND NEW.body_state <> OLD.body_state THEN
        RAISE EXCEPTION 'claimed artifact body cannot transition';
    END IF;
    IF NEW.body_state IS DISTINCT FROM OLD.body_state THEN
        NEW.body_updated = NOW();
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

COMMENT ON COLUMN artifact_version.body_state IS
    'External body lifecycle. cleanup_claimed durably excludes writers and sealers before deletion.';
