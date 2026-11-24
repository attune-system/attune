-- PostgreSQL notifications are visible to all logins to this database. Keep
-- routing fields and operational state here. Retrieve user-supplied content
-- through the corresponding authorized API.
CREATE OR REPLACE FUNCTION _notify_payload_guard(
    channel TEXT,
    full_payload TEXT,
    compact_payload TEXT
) RETURNS VOID AS $body$
DECLARE
    private_fields CONSTANT TEXT[] := ARRAY['trace_tag', 'source_ref', 'progress_message', 'name', 'content_type'];
    operational TEXT := (full_payload::JSONB - private_fields)::TEXT;
BEGIN
    IF octet_length(operational) <= 7000 THEN
        PERFORM pg_notify(channel, operational);
    ELSE
        PERFORM pg_notify(channel, (compact_payload::JSONB - private_fields)::TEXT);
    END IF;
END;
$body$ LANGUAGE plpgsql;

COMMENT ON FUNCTION _notify_payload_guard(TEXT, TEXT, TEXT) IS
    'Emits operational notification fields with a 7000-byte budget for the full envelope.';

CREATE OR REPLACE FUNCTION notify_work_queue_created() RETURNS TRIGGER AS $body$
BEGIN
    PERFORM pg_notify('work_queue_created', json_build_object(
        'entity_type', 'work_queue',
        'entity_id', NEW.id,
        'id', NEW.id,
        'ref', NEW.ref,
        'pack_ref', NEW.pack_ref,
        'is_adhoc', NEW.is_adhoc,
        'enabled', NEW.enabled,
        'dispatch_action_ref', NEW.dispatch_action_ref,
        'default_priority', NEW.default_priority,
        'allow_pending_update', NEW.allow_pending_update,
        'update_strategy', NEW.update_strategy,
        'batch_mode', NEW.batch_mode,
        'created', NEW.created,
        'updated', NEW.updated
    )::TEXT);
    RETURN NEW;
END;
$body$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_work_queue_updated() RETURNS TRIGGER AS $body$
BEGIN
    PERFORM pg_notify('work_queue_updated', json_build_object(
        'entity_type', 'work_queue',
        'entity_id', NEW.id,
        'id', NEW.id,
        'ref', NEW.ref,
        'pack_ref', NEW.pack_ref,
        'is_adhoc', NEW.is_adhoc,
        'enabled', NEW.enabled,
        'dispatch_action_ref', NEW.dispatch_action_ref,
        'default_priority', NEW.default_priority,
        'allow_pending_update', NEW.allow_pending_update,
        'update_strategy', NEW.update_strategy,
        'batch_mode', NEW.batch_mode,
        'created', NEW.created,
        'updated', NEW.updated
    )::TEXT);
    RETURN NEW;
END;
$body$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_work_queue_item_created() RETURNS TRIGGER AS $body$
BEGIN
    PERFORM pg_notify('work_queue_item_created', json_build_object(
        'entity_type', 'work_queue_item',
        'entity_id', NEW.id,
        'id', NEW.id,
        'queue', NEW.queue,
        'queue_ref', NEW.queue_ref,
        'priority', NEW.priority,
        'status', NEW.status,
        'requested_by_identity', NEW.requested_by_identity,
        'requested_by_execution', NEW.requested_by_execution,
        'requested_by_enforcement', NEW.requested_by_enforcement,
        'leased_execution', NEW.leased_execution,
        'lease_expires_at', NEW.lease_expires_at,
        'attempt_count', NEW.attempt_count,
        'created', NEW.created,
        'updated', NEW.updated
    )::TEXT);
    RETURN NEW;
END;
$body$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_work_queue_item_updated() RETURNS TRIGGER AS $body$
BEGIN
    PERFORM pg_notify('work_queue_item_updated', json_build_object(
        'entity_type', 'work_queue_item',
        'entity_id', NEW.id,
        'id', NEW.id,
        'queue', NEW.queue,
        'queue_ref', NEW.queue_ref,
        'priority', NEW.priority,
        'status', NEW.status,
        'old_status', OLD.status,
        'requested_by_identity', NEW.requested_by_identity,
        'requested_by_execution', NEW.requested_by_execution,
        'requested_by_enforcement', NEW.requested_by_enforcement,
        'leased_execution', NEW.leased_execution,
        'lease_expires_at', NEW.lease_expires_at,
        'attempt_count', NEW.attempt_count,
        'created', NEW.created,
        'updated', NEW.updated
    )::TEXT);
    RETURN NEW;
END;
$body$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_on_insert() RETURNS TRIGGER AS $body$
BEGIN
    PERFORM pg_notify(NEW.channel, json_build_object(
        'id', NEW.id,
        'entity_type', NEW.entity_type,
        'entity', NEW.entity
    )::TEXT);
    RETURN NEW;
END;
$body$ LANGUAGE plpgsql;
