CREATE TYPE artifact_upload_grant_state_enum AS ENUM ('issued', 'completed', 'expired');

CREATE TABLE artifact_upload_grant (
    id BIGSERIAL PRIMARY KEY,
    token UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    artifact_version BIGINT NOT NULL
        REFERENCES artifact_version(id) ON DELETE CASCADE,
    segment_sequence BIGINT CHECK (segment_sequence IS NULL OR segment_sequence >= 0),
    object_key TEXT NOT NULL UNIQUE,
    expected_size BIGINT NOT NULL CHECK (expected_size >= 0),
    expected_sha256 TEXT NOT NULL CHECK (expected_sha256 ~ '^[0-9a-f]{64}$'),
    content_type TEXT NOT NULL,
    state artifact_upload_grant_state_enum NOT NULL DEFAULT 'issued',
    expires_at TIMESTAMPTZ NOT NULL,
    settle_until TIMESTAMPTZ NOT NULL CHECK (settle_until >= expires_at),
    completed_provider_version TEXT,
    completed_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (
        (state = 'completed' AND completed_provider_version IS NOT NULL AND completed_at IS NOT NULL)
        OR (state <> 'completed' AND completed_provider_version IS NULL AND completed_at IS NULL)
    )
);

CREATE INDEX idx_artifact_upload_grant_issued_expiry
    ON artifact_upload_grant(settle_until, id)
    WHERE state = 'issued';

CREATE UNIQUE INDEX uq_artifact_upload_grant_artifact_body
    ON artifact_upload_grant(artifact_version)
    WHERE segment_sequence IS NULL;

CREATE UNIQUE INDEX uq_artifact_upload_grant_log_segment
    ON artifact_upload_grant(artifact_version, segment_sequence)
    WHERE segment_sequence IS NOT NULL;

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
    WHERE id = NEW.artifact
      AND NEW.version = (
          SELECT MAX(latest.version) FROM artifact_version latest
          WHERE latest.artifact = NEW.artifact
            AND (latest.body_state IS NULL OR latest.body_state = 'ready')
      );
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER update_artifact_upload_grant_updated
    BEFORE UPDATE ON artifact_upload_grant
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE artifact_upload_grant IS
    'Durable authorization and cleanup grace period for direct artifact object uploads';
