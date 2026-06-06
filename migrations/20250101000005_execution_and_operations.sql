-- Migration: Execution and Operations
-- Description: Creates execution, inquiry, rule, worker, and notification tables.
--              Includes retry tracking, worker health views, and helper functions.
--              Consolidates former migrations: execution_system,
--              worker_notification, worker_table, and 20260209 (phase3).
--
--              NOTE: `execution` remains a regular PostgreSQL table. Time-series
--              audit and analytics are handled by `execution_history`.
-- Version: 20250101000005

-- Set search_path for schema isolation
SET search_path TO attune, public;

-- ============================================================================
-- EXECUTION TABLE
-- ============================================================================

CREATE TABLE execution (
    id BIGSERIAL PRIMARY KEY,
    action BIGINT,
    action_ref TEXT NOT NULL,
    config JSONB,
    env_vars JSONB,
    parent BIGINT,
    enforcement BIGINT,
    executor BIGINT,
    permission_set_refs TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    artifact_retention_policy artifact_retention_enum,
    artifact_retention_limit INTEGER,
    worker_selector JSONB,
    worker_tolerations JSONB,
    worker_affinity JSONB,
    worker BIGINT,
    status execution_status_enum NOT NULL DEFAULT 'requested',
    result JSONB,
    started_at TIMESTAMPTZ,         -- set when execution transitions to 'running'
    timeout_seconds INTEGER,        -- snapshotted resolved execution timeout in seconds
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    workflow_def BIGINT,
    workflow_task JSONB,

    -- Retry tracking (baked in from phase 3)
    retry_count INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER,
    retry_reason TEXT,
    original_execution BIGINT,

    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT execution_artifact_retention_limit_positive CHECK (artifact_retention_limit IS NULL OR artifact_retention_limit > 0),
    CONSTRAINT execution_timeout_seconds_positive CHECK (timeout_seconds IS NULL OR timeout_seconds > 0)
);

-- Indexes
CREATE INDEX idx_execution_action ON execution(action);
CREATE INDEX idx_execution_action_ref ON execution(action_ref);
CREATE INDEX idx_execution_parent ON execution(parent);
CREATE INDEX idx_execution_enforcement ON execution(enforcement);
CREATE INDEX idx_execution_executor ON execution(executor);
CREATE INDEX idx_execution_permission_set_refs ON execution USING GIN (permission_set_refs);
CREATE INDEX idx_execution_worker_selector_gin ON execution USING GIN (worker_selector) WHERE worker_selector IS NOT NULL;
CREATE INDEX idx_execution_worker_tolerations_gin ON execution USING GIN (worker_tolerations) WHERE worker_tolerations IS NOT NULL;
CREATE INDEX idx_execution_worker_affinity_gin ON execution USING GIN (worker_affinity) WHERE worker_affinity IS NOT NULL;
CREATE INDEX idx_execution_worker ON execution(worker);
CREATE INDEX idx_execution_status ON execution(status);
CREATE INDEX idx_execution_created ON execution(created DESC);
CREATE INDEX idx_execution_updated ON execution(updated DESC);
CREATE INDEX idx_execution_status_created ON execution(status, created DESC);
CREATE INDEX idx_execution_status_updated ON execution(status, updated DESC);
CREATE INDEX idx_execution_action_status ON execution(action, status);
CREATE INDEX idx_execution_executor_created ON execution(executor, created DESC);
CREATE INDEX idx_execution_worker_created ON execution(worker, created DESC);
CREATE INDEX idx_execution_parent_created ON execution(parent, created DESC);
CREATE INDEX idx_execution_result_gin ON execution USING GIN (result);
CREATE INDEX idx_execution_env_vars_gin ON execution USING GIN (env_vars);
CREATE INDEX idx_execution_original_execution ON execution(original_execution) WHERE original_execution IS NOT NULL;
CREATE INDEX idx_execution_status_retry ON execution(status, retry_count) WHERE status = 'failed' AND retry_count < COALESCE(max_retries, 0);
CREATE INDEX idx_execution_top_level_created
    ON execution (created DESC)
    WHERE parent IS NULL;
CREATE UNIQUE INDEX uq_execution_top_level_enforcement
    ON execution (enforcement)
    WHERE enforcement IS NOT NULL
      AND parent IS NULL
      AND (config IS NULL OR NOT (config ? 'retry_of'));

-- Trigger
CREATE TRIGGER update_execution_updated
    BEFORE UPDATE ON execution
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON COLUMN execution.worker_selector IS
    'Per-execution worker selector override. NULL inherits the action default; an empty object explicitly clears selector requirements.';
COMMENT ON COLUMN execution.worker_tolerations IS
    'Per-execution worker toleration override. NULL inherits the action default; an empty array explicitly clears tolerations.';
COMMENT ON COLUMN execution.worker_affinity IS
    'Per-execution worker affinity override. NULL inherits the action default; an empty object explicitly clears affinity requirements.';

-- Comments
COMMENT ON TABLE execution IS 'Executions represent action runs, supports nested workflows';
COMMENT ON COLUMN execution.action IS 'Action being executed (may be null if action deleted)';
COMMENT ON COLUMN execution.action_ref IS 'Action reference (preserved even if action deleted)';
COMMENT ON COLUMN execution.config IS 'Snapshot of action configuration at execution time';
COMMENT ON COLUMN execution.env_vars IS 'Environment variables for this execution as key-value pairs (string -> string). These are set in the execution environment and are separate from action parameters. Used for execution context, configuration, and non-sensitive metadata.';
COMMENT ON COLUMN execution.parent IS 'Parent execution ID for workflow hierarchies';
COMMENT ON COLUMN execution.enforcement IS 'Enforcement that triggered this execution';
COMMENT ON COLUMN execution.executor IS 'Identity that initiated the execution';
COMMENT ON COLUMN execution.permission_set_refs IS 'Permission set refs embedded in the execution-scoped API token. Empty means the worker omits ATTUNE_API_TOKEN.';
COMMENT ON COLUMN execution.artifact_retention_policy IS 'Optional per-execution override for non-log artifacts created by this execution. NULL inherits the action/sensor default or API default.';
COMMENT ON COLUMN execution.artifact_retention_limit IS 'Optional per-execution override for non-log artifacts created by this execution. NULL inherits the action/sensor default or API default.';
COMMENT ON COLUMN execution.worker IS 'Assigned worker handling this execution';
COMMENT ON COLUMN execution.status IS 'Current execution lifecycle status';
COMMENT ON COLUMN execution.timeout_seconds IS 'Resolved execution timeout in seconds, snapshotted at creation time (explicit override -> workflow task timeout -> action.timeout_seconds -> app default_execution_timeout_seconds). NULL only for legacy rows; the worker applies a defensive fallback in that case.';
COMMENT ON COLUMN execution.result IS 'Execution output/results';
COMMENT ON COLUMN execution.retry_count IS 'Current retry attempt number (0 = first attempt, 1 = first retry, etc.)';
COMMENT ON COLUMN execution.max_retries IS 'Maximum retries for this execution. Copied from action.max_retries at creation time.';
COMMENT ON COLUMN execution.retry_reason IS 'Reason for retry (e.g., "worker_unavailable", "transient_error", "manual_retry")';
COMMENT ON COLUMN execution.original_execution IS 'ID of the original execution if this is a retry. Forms a retry chain.';

-- ============================================================================

-- Store per-entity execution/enforcement secret values outside general-purpose
-- JSON fields. Public response JSON keeps redaction markers at these paths.

CREATE TABLE execution_secret_value (
    id BIGSERIAL PRIMARY KEY,
    entity_type TEXT NOT NULL,
    entity_id BIGINT NOT NULL,
    json_path TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_ref TEXT,
    encrypted_value JSONB NOT NULL,
    encryption_key_hash TEXT,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (entity_type, entity_id, json_path)
);

CREATE INDEX idx_execution_secret_value_entity
    ON execution_secret_value (entity_type, entity_id);

COMMENT ON TABLE execution_secret_value IS 'Encrypted secret-backed parameter values associated with executions and enforcements';
COMMENT ON COLUMN execution_secret_value.entity_type IS 'Owning entity type for this secret value record (for example execution or enforcement)';
COMMENT ON COLUMN execution_secret_value.entity_id IS 'Owning entity ID for this secret value record';
COMMENT ON COLUMN execution_secret_value.json_path IS 'JSON path within the public payload where a redaction marker is exposed';
COMMENT ON COLUMN execution_secret_value.source_kind IS 'Origin of the secret value (for example key, literal, or inherited)';
COMMENT ON COLUMN execution_secret_value.source_ref IS 'Optional source reference identifying the originating secret record';
COMMENT ON COLUMN execution_secret_value.encrypted_value IS 'Encrypted JSON value used to restore the original secret-backed parameter';
COMMENT ON COLUMN execution_secret_value.encryption_key_hash IS 'Hash of the encryption key used for the stored encrypted value';

-- ============================================================================

-- ============================================================================
-- INQUIRY TABLE
-- ============================================================================

CREATE TABLE inquiry (
    id BIGSERIAL PRIMARY KEY,
    execution BIGINT NOT NULL,
    prompt TEXT NOT NULL,
    response_schema JSONB,
    assigned_to BIGINT REFERENCES identity(id) ON DELETE SET NULL,
    status inquiry_status_enum NOT NULL DEFAULT 'pending',
    response JSONB,
    timeout_at TIMESTAMPTZ,
    responded_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes
CREATE UNIQUE INDEX uq_inquiry_execution ON inquiry(execution) WHERE execution IS NOT NULL;
CREATE INDEX idx_inquiry_assigned_to ON inquiry(assigned_to);
CREATE INDEX idx_inquiry_status ON inquiry(status);
CREATE INDEX idx_inquiry_timeout_at ON inquiry(timeout_at) WHERE timeout_at IS NOT NULL;
CREATE INDEX idx_inquiry_created ON inquiry(created DESC);
CREATE INDEX idx_inquiry_status_created ON inquiry(status, created DESC);
CREATE INDEX idx_inquiry_assigned_status ON inquiry(assigned_to, status);
CREATE INDEX idx_inquiry_execution_status ON inquiry(execution, status);
CREATE INDEX idx_inquiry_response_gin ON inquiry USING GIN (response);

-- Trigger
CREATE TRIGGER update_inquiry_updated
    BEFORE UPDATE ON inquiry
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

-- Comments
COMMENT ON TABLE inquiry IS 'Inquiries enable human-in-the-loop workflows with async user interactions';
COMMENT ON COLUMN inquiry.execution IS 'Execution that is waiting on this inquiry';

ALTER TABLE execution
    ADD CONSTRAINT execution_action_fkey
    FOREIGN KEY (action) REFERENCES action(id) ON DELETE SET NULL;

ALTER TABLE execution
    ADD CONSTRAINT execution_parent_fkey
    FOREIGN KEY (parent) REFERENCES execution(id) ON DELETE SET NULL;

ALTER TABLE execution
    ADD CONSTRAINT execution_original_execution_fkey
    FOREIGN KEY (original_execution) REFERENCES execution(id) ON DELETE SET NULL;

ALTER TABLE execution
    ADD CONSTRAINT execution_enforcement_fkey
    FOREIGN KEY (enforcement) REFERENCES enforcement(id) ON DELETE SET NULL;

ALTER TABLE execution
    ADD CONSTRAINT execution_executor_fkey
    FOREIGN KEY (executor) REFERENCES identity(id) ON DELETE SET NULL;

ALTER TABLE inquiry
    ADD CONSTRAINT inquiry_execution_fkey
    FOREIGN KEY (execution) REFERENCES execution(id) ON DELETE CASCADE;
COMMENT ON COLUMN inquiry.prompt IS 'Question or prompt text for the user';
COMMENT ON COLUMN inquiry.response_schema IS 'JSON schema defining expected response format';
COMMENT ON COLUMN inquiry.assigned_to IS 'Identity who should respond to this inquiry';
COMMENT ON COLUMN inquiry.status IS 'Current inquiry lifecycle status';
COMMENT ON COLUMN inquiry.response IS 'User response data';
COMMENT ON COLUMN inquiry.timeout_at IS 'When this inquiry expires';
COMMENT ON COLUMN inquiry.responded_at IS 'When the response was received';

-- ============================================================================
-- EXECUTION / INQUIRY NOTIFICATIONS
-- ============================================================================

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

CREATE TRIGGER execution_created_notify
    AFTER INSERT ON execution
    FOR EACH ROW
    EXECUTE FUNCTION notify_execution_created();

CREATE TRIGGER execution_status_changed_notify
    AFTER UPDATE ON execution
    FOR EACH ROW
    EXECUTE FUNCTION notify_execution_status_changed();

COMMENT ON FUNCTION notify_execution_created() IS 'Sends execution creation notifications via PostgreSQL LISTEN/NOTIFY';
COMMENT ON FUNCTION notify_execution_status_changed() IS 'Sends execution status change notifications via PostgreSQL LISTEN/NOTIFY';

CREATE OR REPLACE FUNCTION notify_inquiry_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    payload := json_build_object(
        'entity_type', 'inquiry',
        'entity_id', NEW.id,
        'id', NEW.id,
        'execution', NEW.execution,
        'status', NEW.status,
        'timeout_at', NEW.timeout_at,
        'created', NEW.created
    );

    PERFORM pg_notify('inquiry_created', payload::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_inquiry_responded()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.status = 'responded' AND OLD.status != 'responded' THEN
        payload := json_build_object(
            'entity_type', 'inquiry',
            'entity_id', NEW.id,
            'id', NEW.id,
            'execution', NEW.execution,
            'status', NEW.status,
            'updated', NEW.updated
        );

        PERFORM pg_notify('inquiry_responded', payload::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_inquiry_timeout()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.status = 'timeout' AND OLD.status != 'timeout' THEN
        payload := json_build_object(
            'entity_type', 'inquiry',
            'entity_id', NEW.id,
            'id', NEW.id,
            'execution', NEW.execution,
            'status', NEW.status,
            'timeout_at', NEW.timeout_at,
            'updated', NEW.updated
        );

        PERFORM pg_notify('inquiry_timeout', payload::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER inquiry_created_notify
    AFTER INSERT ON inquiry
    FOR EACH ROW
    EXECUTE FUNCTION notify_inquiry_created();

CREATE TRIGGER inquiry_responded_notify
    AFTER UPDATE ON inquiry
    FOR EACH ROW
    EXECUTE FUNCTION notify_inquiry_responded();

CREATE TRIGGER inquiry_timeout_notify
    AFTER UPDATE ON inquiry
    FOR EACH ROW
    EXECUTE FUNCTION notify_inquiry_timeout();

COMMENT ON FUNCTION notify_inquiry_created() IS 'Sends inquiry creation notifications via PostgreSQL LISTEN/NOTIFY';
COMMENT ON FUNCTION notify_inquiry_responded() IS 'Sends inquiry response notifications via PostgreSQL LISTEN/NOTIFY';
COMMENT ON FUNCTION notify_inquiry_timeout() IS 'Sends inquiry timeout notifications via PostgreSQL LISTEN/NOTIFY';

-- ============================================================================

-- ============================================================================
-- RULE TABLE
-- ============================================================================

CREATE TABLE rule (
    id BIGSERIAL PRIMARY KEY,
    ref TEXT NOT NULL UNIQUE,
    pack BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    pack_ref TEXT NOT NULL,
    label TEXT NOT NULL,
    description TEXT,
    action BIGINT REFERENCES action(id) ON DELETE SET NULL,
    action_ref TEXT NOT NULL,
    trigger BIGINT REFERENCES trigger(id) ON DELETE SET NULL,
    trigger_ref TEXT NOT NULL,
    conditions JSONB NOT NULL DEFAULT '[]'::jsonb,
    action_params JSONB DEFAULT '{}'::jsonb,
    trigger_params JSONB DEFAULT '{}'::jsonb,
    permission_set_refs TEXT[],
    enabled BOOLEAN NOT NULL,
    is_adhoc BOOLEAN NOT NULL DEFAULT FALSE,
    owner_identity BIGINT REFERENCES identity(id) ON DELETE SET NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Constraints
    CONSTRAINT rule_ref_lowercase CHECK (ref = LOWER(ref)),
    CONSTRAINT rule_ref_format CHECK (ref ~ '^[^.]+\.[^.]+$')
);

-- Indexes
CREATE INDEX idx_rule_ref ON rule(ref);
CREATE INDEX idx_rule_pack ON rule(pack);
CREATE INDEX idx_rule_action ON rule(action);
CREATE INDEX idx_rule_trigger ON rule(trigger);
CREATE INDEX idx_rule_enabled ON rule(enabled) WHERE enabled = TRUE;
CREATE INDEX idx_rule_is_adhoc ON rule(is_adhoc) WHERE is_adhoc = true;
CREATE INDEX idx_rule_created ON rule(created DESC);
CREATE INDEX idx_rule_trigger_enabled ON rule(trigger, enabled);
CREATE INDEX idx_rule_action_enabled ON rule(action, enabled);
CREATE INDEX idx_rule_pack_enabled ON rule(pack, enabled);
CREATE INDEX idx_rule_action_params_gin ON rule USING GIN (action_params);
CREATE INDEX idx_rule_trigger_params_gin ON rule USING GIN (trigger_params);
CREATE INDEX idx_rule_permission_set_refs ON rule USING GIN (permission_set_refs) WHERE permission_set_refs IS NOT NULL;
CREATE INDEX idx_rule_owner_identity ON rule(owner_identity);

-- Trigger
CREATE TRIGGER update_rule_updated
    BEFORE UPDATE ON rule
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

-- Comments
COMMENT ON TABLE rule IS 'Rules link triggers to actions with conditions';
COMMENT ON COLUMN rule.ref IS 'Unique rule reference (format: pack.name)';
COMMENT ON COLUMN rule.label IS 'Human-readable rule name';
COMMENT ON COLUMN rule.action IS 'Action to execute when rule triggers (null if action deleted)';
COMMENT ON COLUMN rule.trigger IS 'Trigger that activates this rule (null if trigger deleted)';
COMMENT ON COLUMN rule.conditions IS 'Condition expressions to evaluate before executing action';
COMMENT ON COLUMN rule.action_params IS 'Parameter overrides for the action';
COMMENT ON COLUMN rule.trigger_params IS 'Parameter overrides for the trigger';
COMMENT ON COLUMN rule.permission_set_refs IS 'Optional override for execution-scoped API token permission sets. NULL inherits the action default; empty array forces no token.';
COMMENT ON COLUMN rule.enabled IS 'Whether this rule is active';
COMMENT ON COLUMN rule.is_adhoc IS 'True if rule was manually created (ad-hoc), false if installed from pack';
COMMENT ON COLUMN rule.owner_identity IS 'Identity that registered the rule. Used to attribute rule-triggered executions. NULL for system-loaded rules (init pack loader).';

-- ============================================================================

-- Add foreign key constraints now that rule table exists
ALTER TABLE enforcement
    ADD CONSTRAINT enforcement_rule_fkey
    FOREIGN KEY (rule) REFERENCES rule(id) ON DELETE SET NULL;

ALTER TABLE event
    ADD CONSTRAINT event_rule_fkey
    FOREIGN KEY (rule) REFERENCES rule(id) ON DELETE SET NULL;

-- ============================================================================
-- WORKER TABLE
-- ============================================================================

CREATE TABLE worker (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    worker_type worker_type_enum NOT NULL,
    worker_role worker_role_enum NOT NULL,
    runtime BIGINT REFERENCES runtime(id) ON DELETE SET NULL,
    host TEXT,
    port INTEGER,
    status worker_status_enum NOT NULL DEFAULT 'active',
    capabilities JSONB,
    meta JSONB,
    last_heartbeat TIMESTAMPTZ,
    cordoned BOOLEAN NOT NULL DEFAULT FALSE,
    cordon_reason TEXT,
    cordoned_by BIGINT REFERENCES identity(id) ON DELETE SET NULL,
    cordoned_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes
CREATE INDEX idx_worker_name ON worker(name);
CREATE INDEX idx_worker_type ON worker(worker_type);
CREATE INDEX idx_worker_role ON worker(worker_role);
CREATE INDEX idx_worker_runtime ON worker(runtime);
CREATE INDEX idx_worker_status ON worker(status);
CREATE INDEX idx_worker_last_heartbeat ON worker(last_heartbeat DESC) WHERE last_heartbeat IS NOT NULL;
CREATE INDEX idx_worker_created ON worker(created DESC);
CREATE INDEX idx_worker_status_role ON worker(status, worker_role);
CREATE INDEX idx_worker_capabilities_gin ON worker USING GIN (capabilities);
CREATE INDEX idx_worker_meta_gin ON worker USING GIN (meta);
CREATE INDEX idx_worker_capabilities_health_status ON worker USING GIN ((capabilities -> 'health' -> 'status'));
CREATE INDEX idx_worker_cordoned ON worker(cordoned) WHERE cordoned = TRUE;
CREATE INDEX idx_worker_role_cordoned_status ON worker(worker_role, cordoned, status);

-- Trigger
CREATE TRIGGER update_worker_updated
    BEFORE UPDATE ON worker
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

-- Comments
COMMENT ON TABLE worker IS 'Worker registration and tracking table for action and sensor workers';
COMMENT ON COLUMN worker.name IS 'Unique worker identifier (typically hostname-based)';
COMMENT ON COLUMN worker.worker_type IS 'Worker deployment type (local or remote)';
COMMENT ON COLUMN worker.worker_role IS 'Worker role (action or sensor)';
COMMENT ON COLUMN worker.runtime IS 'Runtime environment this worker supports (optional)';
COMMENT ON COLUMN worker.host IS 'Worker host address';
COMMENT ON COLUMN worker.port IS 'Worker port number';
COMMENT ON COLUMN worker.status IS 'Worker operational status';
COMMENT ON COLUMN worker.capabilities IS 'Worker capabilities (e.g., max_concurrent_executions, supported runtimes)';
COMMENT ON COLUMN worker.meta IS 'Additional worker metadata';
COMMENT ON COLUMN worker.last_heartbeat IS 'Timestamp of last heartbeat from worker';
COMMENT ON COLUMN worker.cordoned IS 'Operator cordon flag: cordoned workers are intentionally unschedulable';
COMMENT ON COLUMN worker.cordon_reason IS 'Optional operator-provided reason for cordoning the worker';
COMMENT ON COLUMN worker.cordoned_by IS 'Identity that last cordoned this worker';
COMMENT ON COLUMN worker.cordoned_at IS 'Timestamp when the worker was last cordoned';

ALTER TABLE execution
    ADD CONSTRAINT execution_worker_fkey
    FOREIGN KEY (worker) REFERENCES worker(id) ON DELETE SET NULL;

-- ============================================================================
-- SENSOR_PROCESS TABLE
-- ============================================================================

CREATE TABLE sensor_process (
    id                           BIGSERIAL PRIMARY KEY,
    sensor                       BIGINT NOT NULL REFERENCES sensor(id) ON DELETE CASCADE,
    sensor_ref                   TEXT NOT NULL,
    worker                       BIGINT NOT NULL REFERENCES worker(id) ON DELETE CASCADE,
    worker_name                  TEXT NOT NULL,
    status                       sensor_process_status_enum NOT NULL DEFAULT 'starting',
    pid                          INTEGER,
    consecutive_failures         INTEGER NOT NULL DEFAULT 0,
    last_exit_code               INTEGER,
    last_signal                  INTEGER,
    last_started_at              TIMESTAMPTZ,
    last_stopped_at              TIMESTAMPTZ,
    next_restart_at              TIMESTAMPTZ,
    stderr_excerpt               TEXT,
    log_artifact_ref             TEXT,
    active_rule_count            INTEGER NOT NULL DEFAULT 0,
    last_alerted_failure_count   INTEGER NOT NULL DEFAULT 0,
    last_alerted_at              TIMESTAMPTZ,
    meta                         JSONB NOT NULL DEFAULT '{}'::jsonb,
    created                      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated                      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT sensor_process_pid_positive CHECK (pid IS NULL OR pid > 0),
    CONSTRAINT sensor_process_failures_nonnegative CHECK (consecutive_failures >= 0),
    CONSTRAINT sensor_process_active_rules_nonnegative CHECK (active_rule_count >= 0),
    CONSTRAINT sensor_process_alerted_failures_nonnegative CHECK (last_alerted_failure_count >= 0)
);

CREATE UNIQUE INDEX ux_sensor_process_sensor_worker
    ON sensor_process(sensor, worker);
CREATE UNIQUE INDEX ux_sensor_process_sensor_ref_worker_name
    ON sensor_process(sensor_ref, worker_name);
CREATE INDEX idx_sensor_process_sensor
    ON sensor_process(sensor);
CREATE INDEX idx_sensor_process_sensor_ref
    ON sensor_process(sensor_ref);
CREATE INDEX idx_sensor_process_worker
    ON sensor_process(worker);
CREATE INDEX idx_sensor_process_worker_status
    ON sensor_process(worker, status);
CREATE INDEX idx_sensor_process_status
    ON sensor_process(status);
CREATE INDEX idx_sensor_process_next_restart
    ON sensor_process(next_restart_at)
    WHERE status = 'backoff' AND next_restart_at IS NOT NULL;
CREATE INDEX idx_sensor_process_meta_gin
    ON sensor_process USING GIN (meta);

CREATE TRIGGER update_sensor_process_updated
    BEFORE UPDATE ON sensor_process
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE sensor_process IS 'Live durable state for managed pack sensor processes';
COMMENT ON COLUMN sensor_process.sensor_ref IS 'Denormalized sensor ref for lookup and history after sensor changes';
COMMENT ON COLUMN sensor_process.worker_name IS 'Denormalized owning worker name for lookup and history after worker changes';
COMMENT ON COLUMN sensor_process.stderr_excerpt IS 'Recent stderr excerpt captured when a process exits unexpectedly';
COMMENT ON COLUMN sensor_process.log_artifact_ref IS 'Artifact ref for the durable sensor process log stream';
COMMENT ON COLUMN sensor_process.active_rule_count IS 'Number of enabled rules currently associated with this sensor';
COMMENT ON COLUMN sensor_process.last_alerted_failure_count IS 'Failure count most recently included in a supervisor alert';

-- ============================================================================
-- NOTIFICATION TABLE
-- ============================================================================

CREATE TABLE notification (
    id BIGSERIAL PRIMARY KEY,
    channel TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity TEXT NOT NULL,
    activity TEXT NOT NULL,
    state notification_status_enum NOT NULL DEFAULT 'created',
    content JSONB,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes
CREATE INDEX idx_notification_channel ON notification(channel);
CREATE INDEX idx_notification_entity_type ON notification(entity_type);
CREATE INDEX idx_notification_entity ON notification(entity);
CREATE INDEX idx_notification_state ON notification(state);
CREATE INDEX idx_notification_created ON notification(created DESC);
CREATE INDEX idx_notification_channel_state ON notification(channel, state);
CREATE INDEX idx_notification_entity_type_entity ON notification(entity_type, entity);
CREATE INDEX idx_notification_state_created ON notification(state, created DESC);
CREATE INDEX idx_notification_content_gin ON notification USING GIN (content);

-- Trigger
CREATE TRIGGER update_notification_updated
    BEFORE UPDATE ON notification
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

-- Function for pg_notify on notification insert
CREATE OR REPLACE FUNCTION notify_on_insert()
RETURNS TRIGGER AS $$
DECLARE
    payload TEXT;
BEGIN
    -- Build JSON payload with id, entity, and activity
    payload := json_build_object(
        'id', NEW.id,
        'entity_type', NEW.entity_type,
        'entity', NEW.entity,
        'activity', NEW.activity
    )::text;

    -- Send notification to the specified channel
    PERFORM pg_notify(NEW.channel, payload);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Trigger to send pg_notify on notification insert
CREATE TRIGGER notify_on_notification_insert
    AFTER INSERT ON notification
    FOR EACH ROW
    EXECUTE FUNCTION notify_on_insert();

-- Comments
COMMENT ON TABLE notification IS 'System notifications about entity changes for real-time updates';
COMMENT ON COLUMN notification.channel IS 'Notification channel (typically table name)';
COMMENT ON COLUMN notification.entity_type IS 'Type of entity (table name)';
COMMENT ON COLUMN notification.entity IS 'Entity identifier (typically ID or ref)';
COMMENT ON COLUMN notification.activity IS 'Activity type (e.g., "created", "updated", "completed")';
COMMENT ON COLUMN notification.state IS 'Processing state of notification';
COMMENT ON COLUMN notification.content IS 'Optional notification payload data';

-- ============================================================================
-- WORKER HEALTH VIEWS AND FUNCTIONS
-- ============================================================================

-- View for healthy workers (convenience for queries)
CREATE OR REPLACE VIEW healthy_workers AS
SELECT
    w.id,
    w.name,
    w.worker_type,
    w.worker_role,
    w.runtime,
    w.status,
    w.capabilities,
    w.last_heartbeat,
    (w.capabilities -> 'health' ->> 'status')::TEXT as health_status,
    (w.capabilities -> 'health' ->> 'queue_depth')::INTEGER as queue_depth,
    (w.capabilities -> 'health' ->> 'consecutive_failures')::INTEGER as consecutive_failures
FROM worker w
WHERE
    w.status = 'active'
    AND w.last_heartbeat > NOW() - INTERVAL '30 seconds'
    AND (
        -- Healthy if no health info (backward compatible)
        w.capabilities -> 'health' IS NULL
        OR
        -- Or explicitly marked healthy
        w.capabilities -> 'health' ->> 'status' IN ('healthy', 'degraded')
    );

COMMENT ON VIEW healthy_workers IS 'Workers that are active, have fresh heartbeat, and are healthy or degraded (not unhealthy)';

-- Function to get worker queue depth estimate
CREATE OR REPLACE FUNCTION get_worker_queue_depth(worker_id_param BIGINT)
RETURNS INTEGER AS $$
BEGIN
    RETURN (
        SELECT (capabilities -> 'health' ->> 'queue_depth')::INTEGER
        FROM worker
        WHERE id = worker_id_param
    );
END;
$$ LANGUAGE plpgsql STABLE;

COMMENT ON FUNCTION get_worker_queue_depth IS 'Extract current queue depth from worker health metadata';

-- Function to check if execution is retriable
CREATE OR REPLACE FUNCTION is_execution_retriable(execution_id_param BIGINT)
RETURNS BOOLEAN AS $$
DECLARE
    exec_record RECORD;
BEGIN
    SELECT
        e.retry_count,
        e.max_retries,
        e.status
    INTO exec_record
    FROM execution e
    WHERE e.id = execution_id_param;

    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;

    -- Can retry if:
    -- 1. Status is failed
    -- 2. max_retries is set and > 0
    -- 3. retry_count < max_retries
    RETURN (
        exec_record.status = 'failed'
        AND exec_record.max_retries IS NOT NULL
        AND exec_record.max_retries > 0
        AND exec_record.retry_count < exec_record.max_retries
    );
END;
$$ LANGUAGE plpgsql STABLE;

COMMENT ON FUNCTION is_execution_retriable IS 'Check if a failed execution can be automatically retried based on retry limits';
