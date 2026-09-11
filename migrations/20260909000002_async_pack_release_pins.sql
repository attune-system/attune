ALTER TABLE execution
    ADD COLUMN pack_release BIGINT REFERENCES pack_release(id) ON DELETE SET NULL,
    ADD COLUMN pack_release_digest TEXT,
    ADD COLUMN executable_snapshot JSONB,
    ADD CONSTRAINT execution_release_pin_complete CHECK (
        (pack_release IS NULL AND pack_release_digest IS NULL AND executable_snapshot IS NULL)
        OR (pack_release_digest IS NOT NULL AND executable_snapshot IS NOT NULL)
    ),
    ADD CONSTRAINT execution_release_digest_sha256 CHECK (
        pack_release_digest IS NULL OR pack_release_digest ~ '^[0-9a-f]{64}$'
    );

ALTER TABLE enforcement
    ADD COLUMN pack_release BIGINT REFERENCES pack_release(id) ON DELETE SET NULL,
    ADD COLUMN pack_release_digest TEXT,
    ADD COLUMN executable_snapshot JSONB,
    ADD CONSTRAINT enforcement_release_pin_complete CHECK (
        (pack_release IS NULL AND pack_release_digest IS NULL AND executable_snapshot IS NULL)
        OR (pack_release_digest IS NOT NULL AND executable_snapshot IS NOT NULL)
    ),
    ADD CONSTRAINT enforcement_release_digest_sha256 CHECK (
        pack_release_digest IS NULL OR pack_release_digest ~ '^[0-9a-f]{64}$'
    );

ALTER TABLE work_queue_item
    ADD COLUMN pack_release BIGINT REFERENCES pack_release(id) ON DELETE SET NULL,
    ADD COLUMN pack_release_digest TEXT,
    ADD COLUMN executable_snapshot JSONB,
    ADD CONSTRAINT work_queue_item_release_pin_complete CHECK (
        (pack_release IS NULL AND pack_release_digest IS NULL AND executable_snapshot IS NULL)
        OR (pack_release_digest IS NOT NULL AND executable_snapshot IS NOT NULL)
    ),
    ADD CONSTRAINT work_queue_item_release_digest_sha256 CHECK (
        pack_release_digest IS NULL OR pack_release_digest ~ '^[0-9a-f]{64}$'
    );

ALTER TABLE sensor_workload
    ADD COLUMN pack_release BIGINT REFERENCES pack_release(id) ON DELETE SET NULL,
    ADD COLUMN pack_release_digest TEXT,
    ADD COLUMN executable_snapshot JSONB,
    ADD CONSTRAINT sensor_workload_release_pin_complete CHECK (
        (pack_release IS NULL AND pack_release_digest IS NULL AND executable_snapshot IS NULL)
        OR (pack_release_digest IS NOT NULL AND executable_snapshot IS NOT NULL)
    ),
    ADD CONSTRAINT sensor_workload_release_digest_sha256 CHECK (
        pack_release_digest IS NULL OR pack_release_digest ~ '^[0-9a-f]{64}$'
    );

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
                    'trace_tag', NEW.trace_tag,
                    'pack_release', NEW.pack_release,
                    'pack_release_digest', NEW.pack_release_digest,
                    'executable_snapshot', _jsonb_digest_summary(NEW.executable_snapshot)
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
        old_vals := old_vals || jsonb_build_object('env_vars', _jsonb_digest_summary(OLD.env_vars));
        new_vals := new_vals || jsonb_build_object('env_vars', _jsonb_digest_summary(NEW.env_vars));
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
    IF OLD.pack_release IS DISTINCT FROM NEW.pack_release THEN
        changed := array_append(changed, 'pack_release');
        old_vals := old_vals || jsonb_build_object('pack_release', OLD.pack_release);
        new_vals := new_vals || jsonb_build_object('pack_release', NEW.pack_release);
    END IF;
    IF OLD.pack_release_digest IS DISTINCT FROM NEW.pack_release_digest THEN
        changed := array_append(changed, 'pack_release_digest');
        old_vals := old_vals || jsonb_build_object('pack_release_digest', OLD.pack_release_digest);
        new_vals := new_vals || jsonb_build_object('pack_release_digest', NEW.pack_release_digest);
    END IF;
    IF OLD.executable_snapshot IS DISTINCT FROM NEW.executable_snapshot THEN
        changed := array_append(changed, 'executable_snapshot');
        old_vals := old_vals || jsonb_build_object('executable_snapshot', _jsonb_digest_summary(OLD.executable_snapshot));
        new_vals := new_vals || jsonb_build_object('executable_snapshot', _jsonb_digest_summary(NEW.executable_snapshot));
    END IF;

    IF array_length(changed, 1) > 0 THEN
        INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, old_values, new_values)
        VALUES (NOW(), 'UPDATE', NEW.id, NEW.action_ref, changed, old_vals, new_vals);
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

COMMENT ON COLUMN execution.executable_snapshot IS
    'Immutable action, runtime, runtime-version, and workflow metadata selected at creation';
COMMENT ON COLUMN sensor_workload.executable_snapshot IS
    'Immutable sensor and runtime metadata selected for the desired managed sensor';
