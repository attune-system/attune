-- Migration: Workflow System
-- Description: Creates workflow_definition, workflow_execution, and
--              workflow_task_dispatch tables
--              (workflow_task_execution consolidated into execution.workflow_task JSONB)
--
--              NOTE: `execution` remains a regular PostgreSQL table, so
--              workflow_execution.execution, workflow_task_dispatch.execution_id,
--              and execution.workflow_def use normal foreign keys.
-- Version: 20250101000006

-- Set search_path for schema isolation
SET search_path TO attune, public;

-- ============================================================================
-- WORKFLOW DEFINITION TABLE
-- ============================================================================

CREATE TABLE workflow_definition (
    id BIGSERIAL PRIMARY KEY,
    ref VARCHAR(255) NOT NULL UNIQUE,
    pack BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    pack_ref VARCHAR(255) NOT NULL,
    label VARCHAR(255) NOT NULL,
    description TEXT,
    version VARCHAR(50) NOT NULL,
    param_schema JSONB,
    out_schema JSONB,
    definition JSONB NOT NULL,
    tags TEXT[] DEFAULT '{}',
    created TIMESTAMPTZ DEFAULT NOW() NOT NULL,
    updated TIMESTAMPTZ DEFAULT NOW() NOT NULL
);

-- Indexes
CREATE INDEX idx_workflow_def_pack ON workflow_definition(pack);
CREATE INDEX idx_workflow_def_ref ON workflow_definition(ref);
CREATE INDEX idx_workflow_def_tags ON workflow_definition USING gin(tags);

-- Trigger
CREATE TRIGGER update_workflow_definition_updated
    BEFORE UPDATE ON workflow_definition
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

-- Comments
COMMENT ON TABLE workflow_definition IS 'Stores workflow definitions (YAML parsed to JSON)';
COMMENT ON COLUMN workflow_definition.ref IS 'Unique workflow reference (e.g., pack_name.workflow_name)';
COMMENT ON COLUMN workflow_definition.definition IS 'Complete workflow specification including tasks, variables, and transitions';
COMMENT ON COLUMN workflow_definition.param_schema IS 'JSON schema for workflow input parameters';
COMMENT ON COLUMN workflow_definition.out_schema IS 'JSON schema for workflow output';

-- ============================================================================
-- WORKFLOW EXECUTION TABLE
-- ============================================================================

CREATE TABLE workflow_execution (
    id BIGSERIAL PRIMARY KEY,
    -- Runtime executions are retained independently of workflow state and pins.
    -- This lineage ID may deliberately dangle after execution retention.
    execution BIGINT NOT NULL,
    workflow_def BIGINT NOT NULL REFERENCES workflow_definition(id) ON DELETE CASCADE,
    current_tasks TEXT[] DEFAULT '{}',
    completed_tasks TEXT[] DEFAULT '{}',
    failed_tasks TEXT[] DEFAULT '{}',
    skipped_tasks TEXT[] DEFAULT '{}',
    variables JSONB DEFAULT '{}',
    task_graph JSONB NOT NULL,
    status execution_status_enum NOT NULL DEFAULT 'requested',
    error_message TEXT,
    paused BOOLEAN DEFAULT false NOT NULL,
    pause_reason TEXT,
    created TIMESTAMPTZ DEFAULT NOW() NOT NULL,
    updated TIMESTAMPTZ DEFAULT NOW() NOT NULL,
    CONSTRAINT uq_workflow_execution_execution UNIQUE (execution)
);

-- Indexes
CREATE INDEX idx_workflow_exec_workflow_def ON workflow_execution(workflow_def);
CREATE INDEX idx_workflow_exec_status ON workflow_execution(status);
CREATE INDEX idx_workflow_exec_paused ON workflow_execution(paused) WHERE paused = true;

-- Trigger
CREATE TRIGGER update_workflow_execution_updated
    BEFORE UPDATE ON workflow_execution
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

-- Comments
COMMENT ON TABLE workflow_execution IS 'Runtime state tracking for workflow executions.';
COMMENT ON COLUMN workflow_execution.variables IS 'Workflow-scoped variables, updated via publish directives';
COMMENT ON COLUMN workflow_execution.task_graph IS 'Execution graph with dependencies and transitions';
COMMENT ON COLUMN workflow_execution.current_tasks IS 'Array of task names currently executing';
COMMENT ON COLUMN workflow_execution.paused IS 'True if workflow execution is paused (can be resumed)';

-- ============================================================================
-- WORKFLOW TASK DISPATCH TABLE
-- ============================================================================

CREATE TABLE workflow_task_dispatch (
    id BIGSERIAL PRIMARY KEY,
    workflow_execution BIGINT NOT NULL REFERENCES workflow_execution(id) ON DELETE CASCADE,
    task_name TEXT NOT NULL,
    task_index INT,
    execution_id BIGINT,
    created TIMESTAMPTZ DEFAULT NOW() NOT NULL,
    updated TIMESTAMPTZ DEFAULT NOW() NOT NULL
);

CREATE UNIQUE INDEX uq_workflow_task_dispatch_identity
    ON workflow_task_dispatch (
        workflow_execution,
        task_name,
        COALESCE(task_index, -1)
    );

CREATE INDEX idx_workflow_task_dispatch_execution_id
    ON workflow_task_dispatch (execution_id)
    WHERE execution_id IS NOT NULL;

CREATE TRIGGER update_workflow_task_dispatch_updated
    BEFORE UPDATE ON workflow_task_dispatch
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE workflow_task_dispatch IS
    'Durable dedupe/ownership records for workflow child execution dispatch';
COMMENT ON COLUMN workflow_task_dispatch.execution_id IS
    'Associated execution.id';

ALTER TABLE workflow_task_dispatch
    ADD CONSTRAINT workflow_task_dispatch_execution_id_fkey
    FOREIGN KEY (execution_id) REFERENCES execution(id) ON DELETE CASCADE;

-- ============================================================================
-- MODIFY ACTION TABLE - Add Workflow Support
-- ============================================================================

ALTER TABLE action
    ADD COLUMN workflow_def BIGINT REFERENCES workflow_definition(id) ON DELETE CASCADE,
    ADD COLUMN required_worker_runtimes JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN worker_selector JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN worker_tolerations JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN worker_affinity JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE INDEX idx_action_workflow_def ON action(workflow_def);

COMMENT ON COLUMN action.workflow_def IS 'Reference to workflow definition (non-null means this action is a workflow)';
COMMENT ON COLUMN action.required_worker_runtimes IS
    'Additional worker runtime requirements keyed by runtime name/alias with semver constraints; use "*" for any available version (for example {"node": ">=20", "python": "*"}).';
COMMENT ON COLUMN action.worker_selector IS
    'Exact worker label requirements for scheduling; all labels must match the selected worker.';
COMMENT ON COLUMN action.worker_tolerations IS
    'Tolerations that allow the action to schedule onto workers with matching taints.';
COMMENT ON COLUMN action.worker_affinity IS
    'Required and preferred worker label affinity terms plus required anti-affinity terms for executor scheduling.';

ALTER TABLE execution
    ADD CONSTRAINT execution_workflow_def_fkey
    FOREIGN KEY (workflow_def) REFERENCES workflow_definition(id) ON DELETE SET NULL;

-- ============================================================================
-- WORKFLOW VIEWS
-- ============================================================================

CREATE VIEW workflow_execution_summary AS
SELECT
    we.id,
    we.execution,
    wd.ref as workflow_ref,
    wd.label as workflow_label,
    wd.version as workflow_version,
    we.status,
    we.paused,
    array_length(we.current_tasks, 1) as current_task_count,
    array_length(we.completed_tasks, 1) as completed_task_count,
    array_length(we.failed_tasks, 1) as failed_task_count,
    array_length(we.skipped_tasks, 1) as skipped_task_count,
    we.error_message,
    we.created,
    we.updated
FROM workflow_execution we
JOIN workflow_definition wd ON we.workflow_def = wd.id;

COMMENT ON VIEW workflow_execution_summary IS 'Summary view of workflow executions with task counts';

CREATE VIEW workflow_action_link AS
SELECT
    wd.id as workflow_def_id,
    wd.ref as workflow_ref,
    wd.label,
    wd.version,
    a.id as action_id,
    a.ref as action_ref,
    a.pack as pack_id,
    a.pack_ref
FROM workflow_definition wd
LEFT JOIN action a ON a.workflow_def = wd.id;

COMMENT ON VIEW workflow_action_link IS 'Links workflow definitions to their corresponding action records';

-- ============================================================================
-- WORKFLOW EXECUTION NOTIFICATIONS
-- ============================================================================

CREATE OR REPLACE FUNCTION notify_workflow_execution_status_changed()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    payload := json_build_object(
        'entity_type', 'execution',
        'entity_id', NEW.id,
        'id', NEW.id,
        'action_ref', NEW.action_ref,
        'status', NEW.status,
        'old_status', OLD.status,
        'workflow_def', NEW.workflow_def,
        'parent', NEW.parent,
        'created', NEW.created,
        'updated', NEW.updated
    );

    PERFORM pg_notify('workflow_execution_status_changed', payload::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER workflow_execution_status_changed_notify
    AFTER UPDATE ON execution
    FOR EACH ROW
    WHEN (OLD.status IS DISTINCT FROM NEW.status AND NEW.workflow_def IS NOT NULL)
    EXECUTE FUNCTION notify_workflow_execution_status_changed();

COMMENT ON FUNCTION notify_workflow_execution_status_changed() IS 'Sends workflow execution status change notifications via PostgreSQL LISTEN/NOTIFY';
