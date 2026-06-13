-- Migration: 20250101000015_metadata_cache_sync.sql
-- Description: Emits compact metadata cache invalidation/refresh notifications for cacheable metadata tables.

CREATE OR REPLACE FUNCTION notify_metadata_changed()
RETURNS TRIGGER AS $$
DECLARE
    current_row JSONB;
    old_row JSONB;
    payload JSONB;
BEGIN
    current_row := CASE
        WHEN TG_OP = 'DELETE' THEN to_jsonb(OLD)
        ELSE to_jsonb(NEW)
    END;

    old_row := CASE
        WHEN TG_OP IN ('UPDATE', 'DELETE') THEN to_jsonb(OLD)
        ELSE '{}'::jsonb
    END;

    payload := jsonb_build_object(
        'entity', TG_TABLE_NAME,
        'operation', TG_OP,
        'id', (current_row ->> 'id')::BIGINT,
        'ref', current_row ->> 'ref',
        'pack', CASE WHEN current_row ? 'pack' THEN (current_row ->> 'pack')::BIGINT ELSE NULL END,
        'enabled', CASE WHEN current_row ? 'enabled' THEN (current_row ->> 'enabled')::BOOLEAN ELSE NULL END,
        'action', CASE WHEN current_row ? 'action' THEN (current_row ->> 'action')::BIGINT ELSE NULL END,
        'trigger', CASE WHEN current_row ? 'trigger' THEN (current_row ->> 'trigger')::BIGINT ELSE NULL END,
        'sensor', CASE WHEN current_row ? 'sensor' THEN (current_row ->> 'sensor')::BIGINT ELSE NULL END,
        'runtime', CASE WHEN current_row ? 'runtime' THEN (current_row ->> 'runtime')::BIGINT ELSE NULL END,
        'runtime_ref', CASE WHEN current_row ? 'runtime_ref' THEN current_row ->> 'runtime_ref' ELSE NULL END,
        'version', CASE WHEN current_row ? 'version' THEN current_row ->> 'version' ELSE NULL END,
        'workflow_def', CASE WHEN current_row ? 'workflow_def' THEN (current_row ->> 'workflow_def')::BIGINT ELSE NULL END,
        'webhook_key', CASE WHEN current_row ? 'webhook_key' THEN current_row ->> 'webhook_key' ELSE NULL END,
        'name', CASE WHEN current_row ? 'name' THEN current_row ->> 'name' ELSE NULL END,
        'aliases', CASE WHEN current_row ? 'aliases' THEN current_row -> 'aliases' ELSE NULL END,
        'available', CASE WHEN current_row ? 'available' THEN (current_row ->> 'available')::BOOLEAN ELSE NULL END,
        'accepting_new_items', CASE WHEN current_row ? 'accepting_new_items' THEN (current_row ->> 'accepting_new_items')::BOOLEAN ELSE NULL END,
        'old_ref', CASE WHEN old_row ? 'ref' THEN old_row ->> 'ref' ELSE NULL END,
        'old_pack', CASE WHEN old_row ? 'pack' THEN (old_row ->> 'pack')::BIGINT ELSE NULL END,
        'old_enabled', CASE WHEN old_row ? 'enabled' THEN (old_row ->> 'enabled')::BOOLEAN ELSE NULL END,
        'old_action', CASE WHEN old_row ? 'action' THEN (old_row ->> 'action')::BIGINT ELSE NULL END,
        'old_trigger', CASE WHEN old_row ? 'trigger' THEN (old_row ->> 'trigger')::BIGINT ELSE NULL END,
        'old_sensor', CASE WHEN old_row ? 'sensor' THEN (old_row ->> 'sensor')::BIGINT ELSE NULL END,
        'old_runtime', CASE WHEN old_row ? 'runtime' THEN (old_row ->> 'runtime')::BIGINT ELSE NULL END,
        'old_runtime_ref', CASE WHEN old_row ? 'runtime_ref' THEN old_row ->> 'runtime_ref' ELSE NULL END,
        'old_version', CASE WHEN old_row ? 'version' THEN old_row ->> 'version' ELSE NULL END,
        'old_workflow_def', CASE WHEN old_row ? 'workflow_def' THEN (old_row ->> 'workflow_def')::BIGINT ELSE NULL END,
        'old_webhook_key', CASE WHEN old_row ? 'webhook_key' THEN old_row ->> 'webhook_key' ELSE NULL END,
        'old_name', CASE WHEN old_row ? 'name' THEN old_row ->> 'name' ELSE NULL END,
        'old_aliases', CASE WHEN old_row ? 'aliases' THEN old_row -> 'aliases' ELSE NULL END,
        'old_available', CASE WHEN old_row ? 'available' THEN (old_row ->> 'available')::BOOLEAN ELSE NULL END,
        'old_accepting_new_items', CASE WHEN old_row ? 'accepting_new_items' THEN (old_row ->> 'accepting_new_items')::BOOLEAN ELSE NULL END
    );

    PERFORM pg_notify('metadata_changed', payload::TEXT);

    RETURN CASE
        WHEN TG_OP = 'DELETE' THEN OLD
        ELSE NEW
    END;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER action_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON action
    FOR EACH ROW
    EXECUTE FUNCTION notify_metadata_changed();

CREATE TRIGGER rule_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON rule
    FOR EACH ROW
    EXECUTE FUNCTION notify_metadata_changed();

CREATE TRIGGER trigger_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON trigger
    FOR EACH ROW
    EXECUTE FUNCTION notify_metadata_changed();

CREATE TRIGGER sensor_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON sensor
    FOR EACH ROW
    EXECUTE FUNCTION notify_metadata_changed();

CREATE TRIGGER work_queue_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON work_queue
    FOR EACH ROW
    EXECUTE FUNCTION notify_metadata_changed();

CREATE TRIGGER workflow_definition_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON workflow_definition
    FOR EACH ROW
    EXECUTE FUNCTION notify_metadata_changed();

CREATE TRIGGER policy_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON policy
    FOR EACH ROW
    EXECUTE FUNCTION notify_metadata_changed();

CREATE TRIGGER permission_set_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON permission_set
    FOR EACH ROW
    EXECUTE FUNCTION notify_metadata_changed();

CREATE TRIGGER runtime_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON runtime
    FOR EACH ROW
    EXECUTE FUNCTION notify_metadata_changed();

CREATE TRIGGER runtime_version_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON runtime_version
    FOR EACH ROW
    EXECUTE FUNCTION notify_metadata_changed();

COMMENT ON FUNCTION notify_metadata_changed() IS
    'Sends compact metadata cache notifications via PostgreSQL LISTEN/NOTIFY for cacheable metadata tables';

CREATE OR REPLACE FUNCTION notify_permission_assignment_metadata_changed()
RETURNS TRIGGER AS $$
DECLARE
    current_row JSONB;
    old_row JSONB;
    payload JSONB;
BEGIN
    current_row := CASE
        WHEN TG_OP = 'DELETE' THEN to_jsonb(OLD)
        ELSE to_jsonb(NEW)
    END;

    old_row := CASE
        WHEN TG_OP IN ('UPDATE', 'DELETE') THEN to_jsonb(OLD)
        ELSE '{}'::jsonb
    END;

    payload := jsonb_build_object(
        'entity', TG_TABLE_NAME,
        'operation', TG_OP,
        'id', (current_row ->> 'id')::BIGINT,
        'identity', CASE WHEN current_row ? 'identity' THEN (current_row ->> 'identity')::BIGINT ELSE NULL END,
        'old_identity', CASE WHEN old_row ? 'identity' THEN (old_row ->> 'identity')::BIGINT ELSE NULL END,
        'role', CASE WHEN current_row ? 'role' THEN current_row ->> 'role' ELSE NULL END,
        'old_role', CASE WHEN old_row ? 'role' THEN old_row ->> 'role' ELSE NULL END
    );

    PERFORM pg_notify('metadata_changed', payload::TEXT);

    RETURN CASE
        WHEN TG_OP = 'DELETE' THEN OLD
        ELSE NEW
    END;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER permission_assignment_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON permission_assignment
    FOR EACH ROW
    EXECUTE FUNCTION notify_permission_assignment_metadata_changed();

CREATE TRIGGER permission_set_role_assignment_metadata_changed_notify
    AFTER INSERT OR UPDATE OR DELETE ON permission_set_role_assignment
    FOR EACH ROW
    EXECUTE FUNCTION notify_permission_assignment_metadata_changed();

COMMENT ON FUNCTION notify_permission_assignment_metadata_changed() IS
    'Sends metadata cache notifications for permission-set assignment index invalidation';
