-- Migration: Notification payload auth envelope + size guard
-- Description: Adds an additive `auth_mode` envelope marker to LISTEN/NOTIFY
--              payloads and a shared size guard so payloads never exceed the
--              PostgreSQL NOTIFY 8000-byte limit. When a rich payload would be
--              too large, a compact fallback (core keys + auth_mode='deferred')
--              is emitted and the notifier falls back to a DB visibility check.
-- Version: 20250101000020

SET search_path TO attune, public;

-- ============================================================================
-- SHARED SIZE GUARD
-- ============================================================================

-- Emit `full_payload` if it fits within the safe NOTIFY size budget, otherwise
-- emit `compact_payload`. PostgreSQL's NOTIFY payload hard limit is 8000 bytes;
-- 7000 leaves headroom for channel/frame overhead and multibyte encodings.
CREATE OR REPLACE FUNCTION _notify_payload_guard(
    channel TEXT,
    full_payload TEXT,
    compact_payload TEXT
)
RETURNS VOID AS $$
BEGIN
    IF octet_length(full_payload) <= 7000 THEN
        PERFORM pg_notify(channel, full_payload);
    ELSE
        PERFORM pg_notify(channel, compact_payload);
    END IF;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION _notify_payload_guard(TEXT, TEXT, TEXT) IS
    'Emits full NOTIFY payload when within the 7000-byte safe budget, else a compact fallback.';

-- ============================================================================
-- EXECUTION NOTIFICATIONS
-- ============================================================================

CREATE OR REPLACE FUNCTION notify_execution_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    compact JSON;
    enforcement_rule_ref TEXT;
    enforcement_trigger_ref TEXT;
BEGIN
    IF NEW.enforcement IS NOT NULL THEN
        SELECT rule_ref, trigger_ref
        INTO enforcement_rule_ref, enforcement_trigger_ref
        FROM enforcement
        WHERE id = NEW.enforcement;
    END IF;

    payload := json_build_object(
        'entity_type', 'execution',
        'entity_id', NEW.id,
        'id', NEW.id,
        'action_id', NEW.action,
        'action_ref', NEW.action_ref,
        'status', NEW.status,
        'enforcement', NEW.enforcement,
        'rule_ref', enforcement_rule_ref,
        'trigger_ref', enforcement_trigger_ref,
        'parent', NEW.parent,
        'started_at', NEW.started_at,
        'workflow_task', NEW.workflow_task,
        'created', NEW.created,
        'updated', NEW.updated,
        'auth_mode', 'full'
    );

    compact := json_build_object(
        'entity_type', 'execution',
        'entity_id', NEW.id,
        'id', NEW.id,
        'status', NEW.status,
        'auth_mode', 'deferred'
    );

    PERFORM _notify_payload_guard('execution_created', payload::text, compact::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_execution_status_changed()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    compact JSON;
    enforcement_rule_ref TEXT;
    enforcement_trigger_ref TEXT;
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.status IS DISTINCT FROM NEW.status THEN
        IF NEW.enforcement IS NOT NULL THEN
            SELECT rule_ref, trigger_ref
            INTO enforcement_rule_ref, enforcement_trigger_ref
            FROM enforcement
            WHERE id = NEW.enforcement;
        END IF;

        payload := json_build_object(
            'entity_type', 'execution',
            'entity_id', NEW.id,
            'id', NEW.id,
            'action_id', NEW.action,
            'action_ref', NEW.action_ref,
            'status', NEW.status,
            'old_status', OLD.status,
            'enforcement', NEW.enforcement,
            'rule_ref', enforcement_rule_ref,
            'trigger_ref', enforcement_trigger_ref,
            'parent', NEW.parent,
            'started_at', NEW.started_at,
            'workflow_task', NEW.workflow_task,
            'created', NEW.created,
            'updated', NEW.updated,
            'auth_mode', 'full'
        );

        compact := json_build_object(
            'entity_type', 'execution',
            'entity_id', NEW.id,
            'id', NEW.id,
            'status', NEW.status,
            'old_status', OLD.status,
            'auth_mode', 'deferred'
        );

        PERFORM _notify_payload_guard('execution_status_changed', payload::text, compact::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- ============================================================================
-- EVENT NOTIFICATIONS
-- ============================================================================

CREATE OR REPLACE FUNCTION notify_event_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    compact JSON;
BEGIN
    payload := json_build_object(
        'entity_type', 'event',
        'entity_id', NEW.id,
        'id', NEW.id,
        'trigger', NEW.trigger,
        'trigger_ref', NEW.trigger_ref,
        'source', NEW.source,
        'source_ref', NEW.source_ref,
        'rule', NEW.rule,
        'rule_ref', NEW.rule_ref,
        'has_payload', NEW.payload IS NOT NULL,
        'created', NEW.created,
        'auth_mode', 'full'
    );

    compact := json_build_object(
        'entity_type', 'event',
        'entity_id', NEW.id,
        'id', NEW.id,
        'trigger_ref', NEW.trigger_ref,
        'rule_ref', NEW.rule_ref,
        'auth_mode', 'deferred'
    );

    PERFORM _notify_payload_guard('event_created', payload::text, compact::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION notify_event_created() IS 'Sends event creation notifications via PostgreSQL LISTEN/NOTIFY';

-- ============================================================================
-- ENFORCEMENT NOTIFICATIONS
-- ============================================================================

CREATE OR REPLACE FUNCTION notify_enforcement_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    compact JSON;
BEGIN
    payload := json_build_object(
        'entity_type', 'enforcement',
        'entity_id', NEW.id,
        'id', NEW.id,
        'rule', NEW.rule,
        'rule_ref', NEW.rule_ref,
        'trigger_ref', NEW.trigger_ref,
        'event', NEW.event,
        'status', NEW.status,
        'condition', NEW.condition,
        'created', NEW.created,
        'resolved_at', NEW.resolved_at,
        'auth_mode', 'full'
    );

    -- `condition` (JSONB) is unbounded and dropped from the compact fallback.
    compact := json_build_object(
        'entity_type', 'enforcement',
        'entity_id', NEW.id,
        'id', NEW.id,
        'rule', NEW.rule,
        'rule_ref', NEW.rule_ref,
        'trigger_ref', NEW.trigger_ref,
        'event', NEW.event,
        'status', NEW.status,
        'auth_mode', 'deferred'
    );

    PERFORM _notify_payload_guard('enforcement_created', payload::text, compact::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION notify_enforcement_created() IS 'Sends enforcement creation notifications via PostgreSQL LISTEN/NOTIFY';

CREATE OR REPLACE FUNCTION notify_enforcement_status_changed()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    compact JSON;
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.status IS DISTINCT FROM NEW.status THEN
        payload := json_build_object(
            'entity_type', 'enforcement',
            'entity_id', NEW.id,
            'id', NEW.id,
            'rule', NEW.rule,
            'rule_ref', NEW.rule_ref,
            'trigger_ref', NEW.trigger_ref,
            'event', NEW.event,
            'status', NEW.status,
            'old_status', OLD.status,
            'condition', NEW.condition,
            'created', NEW.created,
            'resolved_at', NEW.resolved_at,
            'auth_mode', 'full'
        );

        compact := json_build_object(
            'entity_type', 'enforcement',
            'entity_id', NEW.id,
            'id', NEW.id,
            'rule', NEW.rule,
            'rule_ref', NEW.rule_ref,
            'trigger_ref', NEW.trigger_ref,
            'event', NEW.event,
            'status', NEW.status,
            'old_status', OLD.status,
            'auth_mode', 'deferred'
        );

        PERFORM _notify_payload_guard('enforcement_status_changed', payload::text, compact::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- ============================================================================
-- ARTIFACT NOTIFICATIONS
-- ============================================================================

CREATE OR REPLACE FUNCTION notify_artifact_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    compact JSON;
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
        'created', NEW.created,
        'auth_mode', 'full'
    );

    compact := json_build_object(
        'entity_type', 'artifact',
        'entity_id', NEW.id,
        'id', NEW.id,
        'ref', NEW.ref,
        'type', NEW.type,
        'visibility', NEW.visibility,
        'scope', NEW.scope,
        'owner', NEW.owner,
        'auth_mode', 'deferred'
    );

    PERFORM _notify_payload_guard('artifact_created', payload::text, compact::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_artifact_updated()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    compact JSON;
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
            'updated', NEW.updated,
            'auth_mode', 'full'
        );

        -- `progress_message` is caller-supplied and unbounded; dropped from the
        -- compact fallback.
        compact := json_build_object(
            'entity_type', 'artifact',
            'entity_id', NEW.id,
            'id', NEW.id,
            'ref', NEW.ref,
            'type', NEW.type,
            'visibility', NEW.visibility,
            'scope', NEW.scope,
            'owner', NEW.owner,
            'execution', latest_execution,
            'progress_percent', latest_percent,
            'progress_entries', entry_count,
            'auth_mode', 'deferred'
        );

        PERFORM _notify_payload_guard('artifact_updated', payload::text, compact::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_artifact_version_changed()
RETURNS TRIGGER AS $$
DECLARE
    artifact_row artifact%ROWTYPE;
    payload JSON;
    compact JSON;
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
        'updated', NOW(),
        'auth_mode', 'full'
    );

    compact := json_build_object(
        'entity_type', 'artifact',
        'entity_id', artifact_row.id,
        'id', artifact_row.id,
        'ref', artifact_row.ref,
        'type', artifact_row.type,
        'visibility', artifact_row.visibility,
        'scope', artifact_row.scope,
        'owner', artifact_row.owner,
        'execution', NEW.execution,
        'artifact_version_id', NEW.id,
        'version', NEW.version,
        'progress_percent', latest_percent,
        'progress_entries', entry_count,
        'auth_mode', 'deferred'
    );

    PERFORM _notify_payload_guard('artifact_updated', payload::text, compact::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
