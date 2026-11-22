-- A runtime child credential must retain the API manager that owns its exact
-- attempt even after the worker slot is reassigned.
ALTER TABLE manager_execution_assignment ADD COLUMN child_attempt UUID;
CREATE UNIQUE INDEX manager_execution_child_attempt
    ON manager_execution_assignment (execution, child_attempt) WHERE child_attempt IS NOT NULL;

-- A start receipt is both idempotency evidence and enduring generation scope.
CREATE TABLE manager_sensor_generation (
    workload BIGINT NOT NULL REFERENCES sensor_workload(id) ON DELETE CASCADE,
    generation BIGINT NOT NULL,
    request UUID NOT NULL,
    manager_session BIGINT NOT NULL REFERENCES manager_session(id),
    instance UUID NOT NULL,
    worker BIGINT NOT NULL,
    PRIMARY KEY (workload, generation),
    UNIQUE (manager_session, request)
);
