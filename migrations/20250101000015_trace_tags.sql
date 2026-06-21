-- Migration: Trace tag propagation fields
-- Description: Adds trace tag columns to execution/rule/work_queue and
--              updates execution notification + history trigger functions.
-- Version: 20250101000015

SET search_path TO attune, public;

ALTER TABLE execution
    ADD COLUMN trace_tag TEXT;

ALTER TABLE rule
    ADD COLUMN trace_tag_template TEXT;

ALTER TABLE work_queue
    ADD COLUMN trace_tag_template TEXT;

CREATE INDEX idx_execution_trace_tag
    ON execution(trace_tag)
    WHERE trace_tag IS NOT NULL;

COMMENT ON COLUMN execution.trace_tag IS
    'Immutable trace tag snapshotted at execution creation for cross-component activity correlation.';
COMMENT ON COLUMN rule.trace_tag_template IS
    'Optional template used to resolve trace_tag for executions created from this rule. Defaults to <trigger_ref>.<event_id> when unset.';
COMMENT ON COLUMN work_queue.trace_tag_template IS
    'Optional template used to resolve trace_tag for queue dispatch executions. Defaults to <queue_ref>.<work_item_id> (single) or <queue_ref>.<dispatch_id> (batch) when unset.';

CREATE OR REPLACE FUNCTION notify_execution_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
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
        'updated', NEW.updated
    );

    PERFORM pg_notify('execution_created', payload::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_execution_status_changed()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
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
            'updated', NEW.updated
        );

        PERFORM pg_notify('execution_status_changed', payload::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION record_execution_history()
RETURNS TRIGGER AS $$
DECLARE
    changed TEXT[] := '{}';
    old_vals JSONB := '{}';
    new_vals JSONB := '{}';
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, old_values, new_values)
        VALUES (NOW(), 'INSERT', NEW.id, NEW.action_ref, '{}', NULL,
                jsonb_build_object(
                    'status', NEW.status,
                    'action_ref', NEW.action_ref,
                    'executor', NEW.executor,
                    'worker', NEW.worker,
                    'parent', NEW.parent,
                    'enforcement', NEW.enforcement,
                    'started_at', NEW.started_at,
                    'trace_tag', NEW.trace_tag
                ));
        RETURN NEW;
    END IF;

    IF TG_OP = 'DELETE' THEN
        INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, old_values, new_values)
        VALUES (NOW(), 'DELETE', OLD.id, OLD.action_ref, '{}', NULL, NULL);
        RETURN OLD;
    END IF;

    IF OLD.status IS DISTINCT FROM NEW.status THEN
        changed := array_append(changed, 'status');
        old_vals := old_vals || jsonb_build_object('status', OLD.status);
        new_vals := new_vals || jsonb_build_object('status', NEW.status);
    END IF;

    IF OLD.result IS DISTINCT FROM NEW.result THEN
        changed := array_append(changed, 'result');
        old_vals := old_vals || jsonb_build_object('result', _jsonb_digest_summary(OLD.result));
        new_vals := new_vals || jsonb_build_object('result', _jsonb_digest_summary(NEW.result));
    END IF;

    IF OLD.executor IS DISTINCT FROM NEW.executor THEN
        changed := array_append(changed, 'executor');
        old_vals := old_vals || jsonb_build_object('executor', OLD.executor);
        new_vals := new_vals || jsonb_build_object('executor', NEW.executor);
    END IF;

    IF OLD.worker IS DISTINCT FROM NEW.worker THEN
        changed := array_append(changed, 'worker');
        old_vals := old_vals || jsonb_build_object('worker', OLD.worker);
        new_vals := new_vals || jsonb_build_object('worker', NEW.worker);
    END IF;

    IF OLD.workflow_task IS DISTINCT FROM NEW.workflow_task THEN
        changed := array_append(changed, 'workflow_task');
        old_vals := old_vals || jsonb_build_object('workflow_task', OLD.workflow_task);
        new_vals := new_vals || jsonb_build_object('workflow_task', NEW.workflow_task);
    END IF;

    IF OLD.env_vars IS DISTINCT FROM NEW.env_vars THEN
        changed := array_append(changed, 'env_vars');
        old_vals := old_vals || jsonb_build_object('env_vars', OLD.env_vars);
        new_vals := new_vals || jsonb_build_object('env_vars', NEW.env_vars);
    END IF;

    IF OLD.started_at IS DISTINCT FROM NEW.started_at THEN
        changed := array_append(changed, 'started_at');
        old_vals := old_vals || jsonb_build_object('started_at', OLD.started_at);
        new_vals := new_vals || jsonb_build_object('started_at', NEW.started_at);
    END IF;

    IF OLD.trace_tag IS DISTINCT FROM NEW.trace_tag THEN
        changed := array_append(changed, 'trace_tag');
        old_vals := old_vals || jsonb_build_object('trace_tag', OLD.trace_tag);
        new_vals := new_vals || jsonb_build_object('trace_tag', NEW.trace_tag);
    END IF;

    IF array_length(changed, 1) > 0 THEN
        INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, old_values, new_values)
        VALUES (NOW(), 'UPDATE', NEW.id, NEW.action_ref, changed, old_vals, new_vals);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
