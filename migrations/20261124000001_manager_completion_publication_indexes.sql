-- Keep new submission lookups independent of the unprocessed replay backlog.
CREATE INDEX manager_execution_completion_initial_ready
    ON manager_execution_completion (available_at, assignment)
    WHERE processed_at IS NULL AND published_at IS NULL AND claim IS NULL;

-- A replay probe must also be cheap when all due work is a first publication.
CREATE INDEX manager_execution_completion_replay_ready
    ON manager_execution_completion (available_at, assignment)
    WHERE processed_at IS NULL AND (published_at IS NOT NULL OR claim IS NOT NULL);

DROP INDEX manager_execution_completion_ready;
