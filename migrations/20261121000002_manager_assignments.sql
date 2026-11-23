-- The action transport is selected by the current server-owned registration.
CREATE TABLE manager_action_worker (
    worker BIGINT PRIMARY KEY REFERENCES worker(id) ON DELETE CASCADE,
    manager_session BIGINT NOT NULL UNIQUE REFERENCES manager_session(id),
    draining BOOLEAN NOT NULL DEFAULT FALSE
);

-- Execution retention is independent of assignment and completion retention.
CREATE TABLE manager_execution_assignment (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    execution BIGINT NOT NULL,
    worker BIGINT NOT NULL,
    manager_session BIGINT NOT NULL REFERENCES manager_session(id),
    instance UUID NOT NULL,
    scheduled_at TIMESTAMPTZ NOT NULL,
    pack_release BIGINT NOT NULL,
    pack_release_digest TEXT NOT NULL,
    deadline TIMESTAMPTZ NOT NULL,
    claim_request UUID,
    closed_at TIMESTAMPTZ,
    finish_digest TEXT,
    final_status execution_status_enum,
    CONSTRAINT manager_execution_finish_closed CHECK (
        finish_digest IS NULL OR (claim_request IS NOT NULL AND closed_at IS NOT NULL AND final_status IS NOT NULL)
    ),
    CONSTRAINT manager_execution_status_closed CHECK (final_status IS NULL OR closed_at IS NOT NULL)
);

CREATE UNIQUE INDEX manager_execution_open_assignment
    ON manager_execution_assignment (execution) WHERE closed_at IS NULL;
CREATE INDEX manager_execution_wait_offers
    ON manager_execution_assignment (manager_session, id)
    WHERE claim_request IS NULL AND closed_at IS NULL;
CREATE INDEX manager_execution_recover
    ON manager_execution_assignment (deadline, id) WHERE closed_at IS NULL;
CREATE INDEX manager_execution_assignment_by_execution
    ON manager_execution_assignment (execution, id);

CREATE TABLE manager_execution_completion (
    assignment BIGINT PRIMARY KEY REFERENCES manager_execution_assignment(id) ON DELETE CASCADE,
    message_id UUID NOT NULL UNIQUE,
    execution BIGINT NOT NULL,
    action BIGINT NOT NULL,
    action_ref TEXT NOT NULL,
    status execution_status_enum NOT NULL CHECK (status IN ('completed', 'failed', 'timeout', 'cancelled', 'abandoned')),
    completed_at TIMESTAMPTZ NOT NULL,
    available_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    claim UUID,
    published_at TIMESTAMPTZ
);

CREATE INDEX manager_execution_completion_ready
    ON manager_execution_completion (available_at, assignment) WHERE published_at IS NULL;
