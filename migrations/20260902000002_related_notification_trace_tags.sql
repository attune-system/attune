-- Add trace tags to event, enforcement, and work queue item notifications.

SET search_path TO attune, public;

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
        'trace_tag', NEW.trace_tag,
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
        'trace_tag', NEW.trace_tag,
        'auth_mode', 'deferred'
    );

    PERFORM _notify_payload_guard('event_created', payload::text, compact::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_enforcement_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    compact JSON;
    enforcement_trace_tag TEXT;
BEGIN
    SELECT trace_tag
    INTO enforcement_trace_tag
    FROM execution
    WHERE enforcement = NEW.id
      AND trace_tag IS NOT NULL
    ORDER BY created ASC, id ASC
    LIMIT 1;

    IF enforcement_trace_tag IS NULL AND NEW.event IS NOT NULL THEN
        SELECT trace_tag
        INTO enforcement_trace_tag
        FROM event
        WHERE id = NEW.event;
    END IF;

    payload := json_build_object(
        'entity_type', 'enforcement',
        'entity_id', NEW.id,
        'id', NEW.id,
        'rule', NEW.rule,
        'rule_ref', NEW.rule_ref,
        'trigger_ref', NEW.trigger_ref,
        'event', NEW.event,
        'status', NEW.status,
        'trace_tag', enforcement_trace_tag,
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
        'trace_tag', enforcement_trace_tag,
        'auth_mode', 'deferred'
    );

    PERFORM _notify_payload_guard('enforcement_created', payload::text, compact::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_enforcement_status_changed()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    compact JSON;
    enforcement_trace_tag TEXT;
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.status IS DISTINCT FROM NEW.status THEN
        SELECT trace_tag
        INTO enforcement_trace_tag
        FROM execution
        WHERE enforcement = NEW.id
          AND trace_tag IS NOT NULL
        ORDER BY created ASC, id ASC
        LIMIT 1;

        IF enforcement_trace_tag IS NULL AND NEW.event IS NOT NULL THEN
            SELECT trace_tag
            INTO enforcement_trace_tag
            FROM event
            WHERE id = NEW.event;
        END IF;

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
            'trace_tag', enforcement_trace_tag,
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
            'trace_tag', enforcement_trace_tag,
            'auth_mode', 'deferred'
        );

        PERFORM _notify_payload_guard('enforcement_status_changed', payload::text, compact::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_work_queue_item_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    payload := json_build_object(
        'entity_type', 'work_queue_item',
        'entity_id', NEW.id,
        'id', NEW.id,
        'queue', NEW.queue,
        'queue_ref', NEW.queue_ref,
        'item_key', NEW.item_key,
        'priority', NEW.priority,
        'status', NEW.status,
        'trace_tag', NEW.trace_tag,
        'enqueue_source', NEW.enqueue_source,
        'requested_by_identity', NEW.requested_by_identity,
        'requested_by_execution', NEW.requested_by_execution,
        'requested_by_enforcement', NEW.requested_by_enforcement,
        'leased_execution', NEW.leased_execution,
        'lease_expires_at', NEW.lease_expires_at,
        'attempt_count', NEW.attempt_count,
        'created', NEW.created,
        'updated', NEW.updated
    );

    PERFORM pg_notify('work_queue_item_created', payload::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_work_queue_item_updated()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        payload := json_build_object(
            'entity_type', 'work_queue_item',
            'entity_id', NEW.id,
            'id', NEW.id,
            'queue', NEW.queue,
            'queue_ref', NEW.queue_ref,
            'item_key', NEW.item_key,
            'priority', NEW.priority,
            'status', NEW.status,
            'old_status', OLD.status,
            'trace_tag', NEW.trace_tag,
            'enqueue_source', NEW.enqueue_source,
            'requested_by_identity', NEW.requested_by_identity,
            'requested_by_execution', NEW.requested_by_execution,
            'requested_by_enforcement', NEW.requested_by_enforcement,
            'leased_execution', NEW.leased_execution,
            'lease_expires_at', NEW.lease_expires_at,
            'attempt_count', NEW.attempt_count,
            'created', NEW.created,
            'updated', NEW.updated
        );

        PERFORM pg_notify('work_queue_item_updated', payload::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
