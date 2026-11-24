-- Keep new submission lookups independent of the unprocessed replay backlog.
CREATE INDEX manager_execution_completion_initial_ready
    ON manager_execution_completion (available_at, assignment)
    WHERE processed_at IS NULL AND published_at IS NULL AND claim IS NULL;
