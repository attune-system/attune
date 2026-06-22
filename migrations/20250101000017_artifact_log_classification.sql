-- Migration: Artifact log classification
-- Description: Adds explicit artifact classification so runtime logs remain private
--              artifact-backed source-of-truth while external observability uses
--              metadata-only lifecycle signals.
-- Version: 20250101000017

SET search_path TO attune, public;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_type t
        JOIN pg_namespace n ON n.oid = t.typnamespace
        WHERE t.typname = 'artifact_classification_enum'
          AND n.nspname = current_schema()
    ) THEN
        CREATE TYPE artifact_classification_enum AS ENUM ('general', 'runtime_log');
    END IF;
END $$;

ALTER TABLE artifact
    ADD COLUMN IF NOT EXISTS classification artifact_classification_enum NOT NULL DEFAULT 'general';

UPDATE artifact
SET classification = 'runtime_log',
    visibility = 'private'
WHERE type = 'file_text'
  AND (
        ref LIKE '%.stdout.log'
        OR ref LIKE '%.stderr.log'
        OR (
            ref LIKE 'sensor.%'
            AND (ref LIKE '%.stdout' OR ref LIKE '%.stderr')
        )
    );

CREATE INDEX IF NOT EXISTS idx_artifact_classification ON artifact(classification);
CREATE INDEX IF NOT EXISTS idx_artifact_classification_created ON artifact(classification, created DESC);

COMMENT ON TYPE artifact_classification_enum IS 'High-level artifact classification (general or runtime_log)';
COMMENT ON COLUMN artifact.classification IS 'Distinguishes general artifacts from runtime log artifacts used as the private source of truth for stdout/stderr';

CREATE OR REPLACE FUNCTION notify_artifact_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    payload := json_build_object(
        'entity_type', 'artifact',
        'entity_id', NEW.id,
        'id', NEW.id,
        'ref', NEW.ref,
        'type', NEW.type,
        'visibility', NEW.visibility,
        'classification', NEW.classification,
        'name', NEW.name,
        'scope', NEW.scope,
        'owner', NEW.owner,
        'content_type', NEW.content_type,
        'size_bytes', NEW.size_bytes,
        'created', NEW.created
    );

    PERFORM pg_notify('artifact_created', payload::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_artifact_updated()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    latest_percent DOUBLE PRECISION;
    latest_message TEXT;
    entry_count INTEGER;
    latest_execution BIGINT;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        SELECT av.execution
        INTO latest_execution
        FROM artifact_version av
        WHERE av.artifact = NEW.id
        ORDER BY av.version DESC
        LIMIT 1;

        IF NEW.type = 'progress' AND NEW.data IS NOT NULL AND jsonb_typeof(NEW.data) = 'array' THEN
            entry_count := jsonb_array_length(NEW.data);
            IF entry_count > 0 THEN
                latest_percent := (NEW.data -> (entry_count - 1) ->> 'percent')::DOUBLE PRECISION;
                latest_message := NEW.data -> (entry_count - 1) ->> 'message';
            END IF;
        END IF;

        payload := json_build_object(
            'entity_type', 'artifact',
            'entity_id', NEW.id,
            'id', NEW.id,
            'ref', NEW.ref,
            'type', NEW.type,
            'visibility', NEW.visibility,
            'classification', NEW.classification,
            'name', NEW.name,
            'scope', NEW.scope,
            'owner', NEW.owner,
            'content_type', NEW.content_type,
            'size_bytes', NEW.size_bytes,
            'execution', latest_execution,
            'progress_percent', latest_percent,
            'progress_message', latest_message,
            'progress_entries', entry_count,
            'created', NEW.created,
            'updated', NEW.updated
        );

        PERFORM pg_notify('artifact_updated', payload::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_artifact_version_changed()
RETURNS TRIGGER AS $$
DECLARE
    artifact_row artifact%ROWTYPE;
    payload JSON;
    latest_percent DOUBLE PRECISION;
    latest_message TEXT;
    entry_count INTEGER;
BEGIN
    SELECT *
    INTO artifact_row
    FROM artifact
    WHERE id = NEW.artifact;

    IF NOT FOUND THEN
        RETURN NEW;
    END IF;

    IF artifact_row.type = 'progress'
        AND artifact_row.data IS NOT NULL
        AND jsonb_typeof(artifact_row.data) = 'array'
    THEN
        entry_count := jsonb_array_length(artifact_row.data);
        IF entry_count > 0 THEN
            latest_percent := (artifact_row.data -> (entry_count - 1) ->> 'percent')::DOUBLE PRECISION;
            latest_message := artifact_row.data -> (entry_count - 1) ->> 'message';
        END IF;
    END IF;

    payload := json_build_object(
        'entity_type', 'artifact',
        'entity_id', artifact_row.id,
        'id', artifact_row.id,
        'ref', artifact_row.ref,
        'type', artifact_row.type,
        'visibility', artifact_row.visibility,
        'classification', artifact_row.classification,
        'name', artifact_row.name,
        'scope', artifact_row.scope,
        'owner', artifact_row.owner,
        'content_type', COALESCE(NEW.content_type, artifact_row.content_type),
        'size_bytes', COALESCE(NEW.size_bytes, artifact_row.size_bytes),
        'execution', NEW.execution,
        'artifact_version_id', NEW.id,
        'version', NEW.version,
        'progress_percent', latest_percent,
        'progress_message', latest_message,
        'progress_entries', entry_count,
        'created', artifact_row.created,
        'updated', NOW()
    );

    PERFORM pg_notify('artifact_updated', payload::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
