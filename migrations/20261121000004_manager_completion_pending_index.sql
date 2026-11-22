DROP INDEX manager_execution_completion_ready;
CREATE INDEX manager_execution_completion_ready
    ON manager_execution_completion (available_at, assignment) WHERE processed_at IS NULL;
