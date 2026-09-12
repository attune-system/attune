CREATE TYPE log_stream_backend_enum AS ENUM ('object_segments', 'shared_file');

ALTER TABLE log_stream
    ADD COLUMN backend log_stream_backend_enum NOT NULL DEFAULT 'object_segments';

ALTER TABLE artifact_version
    DROP CONSTRAINT ck_artifact_version_object_metadata,
    ADD CONSTRAINT ck_artifact_version_object_metadata CHECK (
        (body_state IS NULL AND object_key IS NULL AND provider_version IS NULL AND sha256 IS NULL)
        OR (
            body_state = 'pending'
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

COMMENT ON TYPE log_stream_backend_enum IS
    'Durable byte placement selected when a log stream is created.';
COMMENT ON COLUMN log_stream.backend IS
    'Authoritative reader backend. Existing streams use object_segments.';

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
              AND NOT EXISTS (
                  SELECT 1 FROM log_stream stream
                  WHERE stream.artifact_version = stale.id
                    AND stream.backend = 'shared_file'
              )
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
