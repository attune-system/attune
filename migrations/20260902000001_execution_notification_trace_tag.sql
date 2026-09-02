-- Restore trace tags dropped when the notification size envelope replaced the
-- execution trigger functions.

SET search_path TO attune, public;

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
        'trace_tag', NEW.trace_tag,
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
        'trace_tag', NEW.trace_tag,
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
            'trace_tag', NEW.trace_tag,
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
            'trace_tag', NEW.trace_tag,
            'auth_mode', 'deferred'
        );

        PERFORM _notify_payload_guard('execution_status_changed', payload::text, compact::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
