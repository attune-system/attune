-- Durable pre-dispatch workflow waits and action-owned inquiry scope.

DO $$ BEGIN
    CREATE TYPE workflow_task_wait_kind_enum AS ENUM ('inquiry');
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

DO $$ BEGIN
    CREATE TYPE workflow_task_wait_state_enum AS ENUM (
        'waiting',
        'failed',
        'timed_out',
        'cancelled',
        'released'
    );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

DROP INDEX IF EXISTS uq_inquiry_execution;

ALTER TABLE inquiry
    DROP CONSTRAINT IF EXISTS inquiry_execution_fkey,
    ADD COLUMN workflow_execution BIGINT REFERENCES workflow_execution(id) ON DELETE CASCADE,
    ADD COLUMN workflow_task_name TEXT,
    ADD COLUMN action_attempt_family BIGINT,
    ADD COLUMN purpose TEXT,
    ADD COLUMN timeout_seconds BIGINT,
    ADD COLUMN responded_by BIGINT REFERENCES identity(id) ON DELETE SET NULL,
    ADD COLUMN provider_actor JSONB,
    ADD CONSTRAINT inquiry_workflow_scope_complete CHECK (
        (workflow_execution IS NULL AND workflow_task_name IS NULL AND action_attempt_family IS NULL AND purpose IS NULL)
        OR
        (workflow_execution IS NOT NULL AND workflow_task_name IS NOT NULL AND action_attempt_family IS NOT NULL AND purpose IS NOT NULL)
    ),
    ADD CONSTRAINT inquiry_purpose_nonempty CHECK (
        purpose IS NULL OR length(trim(purpose)) > 0
    ),
    ADD CONSTRAINT inquiry_timeout_seconds_positive CHECK (
        timeout_seconds IS NULL OR timeout_seconds > 0
    );

CREATE UNIQUE INDEX uq_inquiry_workflow_task_purpose
    ON inquiry(workflow_execution, workflow_task_name, action_attempt_family, purpose)
    WHERE workflow_execution IS NOT NULL;
CREATE INDEX idx_inquiry_workflow_status
    ON inquiry(workflow_execution, status)
    WHERE workflow_execution IS NOT NULL;
CREATE INDEX idx_inquiry_workflow_task
    ON inquiry(workflow_execution, workflow_task_name)
    WHERE workflow_execution IS NOT NULL;

COMMENT ON COLUMN inquiry.execution IS 'Execution that created this inquiry; plain BIGINT because execution is a hypertable';
COMMENT ON COLUMN inquiry.workflow_execution IS 'Workflow scope derived from the creator execution';
COMMENT ON COLUMN inquiry.workflow_task_name IS 'Creator workflow task name derived from the creator execution';
COMMENT ON COLUMN inquiry.action_attempt_family IS 'Stable original execution ID used for idempotency across action retries';
COMMENT ON COLUMN inquiry.purpose IS 'Pack-supplied inquiry purpose unique within the workflow task attempt family';
COMMENT ON COLUMN inquiry.timeout_seconds IS 'Immutable relative timeout used to compare idempotent create requests';
COMMENT ON COLUMN inquiry.responded_by IS 'Attune identity that submitted the accepted response';
COMMENT ON COLUMN inquiry.provider_actor IS 'Non-secret provider actor evidence for an integration response';

CREATE TABLE workflow_task_wait (
    id BIGSERIAL PRIMARY KEY,
    workflow_execution BIGINT NOT NULL REFERENCES workflow_execution(id) ON DELETE CASCADE,
    task_name TEXT NOT NULL,
    kind workflow_task_wait_kind_enum NOT NULL,
    state workflow_task_wait_state_enum NOT NULL DEFAULT 'waiting',
    inquiry BIGINT NOT NULL REFERENCES inquiry(id) ON DELETE RESTRICT,
    result JSONB,
    resolved_at TIMESTAMPTZ,
    released_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_workflow_task_wait_identity UNIQUE (workflow_execution, task_name),
    CONSTRAINT workflow_task_wait_resolution_consistent CHECK (
        (state = 'waiting' AND resolved_at IS NULL AND released_at IS NULL)
        OR
        (state IN ('failed', 'timed_out', 'cancelled') AND resolved_at IS NOT NULL AND released_at IS NULL)
        OR
        (state = 'released' AND resolved_at IS NOT NULL AND released_at IS NOT NULL)
    )
);

CREATE INDEX idx_workflow_task_wait_inquiry_state
    ON workflow_task_wait(inquiry, state);
CREATE INDEX idx_workflow_task_wait_workflow_state
    ON workflow_task_wait(workflow_execution, state);
CREATE INDEX idx_workflow_task_wait_waiting
    ON workflow_task_wait(workflow_execution, id)
    WHERE state = 'waiting';

CREATE TRIGGER update_workflow_task_wait_updated
    BEFORE UPDATE ON workflow_task_wait
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE workflow_task_wait IS 'Durable prerequisites resolved before workflow child execution creation';
COMMENT ON COLUMN workflow_task_wait.inquiry IS 'Inquiry whose terminal state controls release of the guarded task';
COMMENT ON COLUMN workflow_task_wait.result IS 'Safe logical outcome used when no guarded child execution is created';

DELETE FROM intrinsic_handler WHERE ref = 'attune.inquiry/v1';
