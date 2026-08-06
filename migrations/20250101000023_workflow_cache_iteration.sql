-- Migration: Durable workflow cache iteration
-- Description: Persists cache scan progress and pins generations used by workflow tasks.
-- Version: 20250101000023

SET search_path TO attune, public;

CREATE TYPE workflow_cache_iteration_state_enum AS ENUM (
    'scanning',
    'completed',
    'failed',
    'cancelled'
);

CREATE TABLE workflow_cache_iteration (
    id BIGSERIAL PRIMARY KEY,
    workflow_execution BIGINT NOT NULL REFERENCES workflow_execution(id) ON DELETE CASCADE,
    task_name TEXT NOT NULL,
    namespace BIGINT NOT NULL,
    generation BIGINT NOT NULL,
    state workflow_cache_iteration_state_enum NOT NULL DEFAULT 'scanning',
    last_external_id TEXT COLLATE "C",
    next_batch_index BIGINT NOT NULL DEFAULT 0,
    scanned_count BIGINT NOT NULL DEFAULT 0,
    dispatched_count BIGINT NOT NULL DEFAULT 0,
    page_size INTEGER NOT NULL,
    batch_size INTEGER NOT NULL,
    concurrency INTEGER NOT NULL,
    completed_at TIMESTAMPTZ,
    error_summary TEXT,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT workflow_cache_iteration_workflow_task_unique
        UNIQUE (workflow_execution, task_name),
    CONSTRAINT workflow_cache_iteration_generation_namespace_fkey
        FOREIGN KEY (generation, namespace)
        REFERENCES cache_generation(id, namespace) ON DELETE CASCADE,
    CONSTRAINT workflow_cache_iteration_task_name_nonempty CHECK (BTRIM(task_name) <> ''),
    CONSTRAINT workflow_cache_iteration_task_name_length CHECK (OCTET_LENGTH(task_name) <= 1024),
    CONSTRAINT workflow_cache_iteration_next_batch_nonnegative CHECK (next_batch_index >= 0),
    CONSTRAINT workflow_cache_iteration_scanned_nonnegative CHECK (scanned_count >= 0),
    CONSTRAINT workflow_cache_iteration_dispatched_nonnegative CHECK (dispatched_count >= 0),
    CONSTRAINT workflow_cache_iteration_page_size_bounds CHECK (page_size BETWEEN 1 AND 1000),
    CONSTRAINT workflow_cache_iteration_batch_size_bounds CHECK (batch_size BETWEEN 1 AND 1000),
    CONSTRAINT workflow_cache_iteration_concurrency_bounds CHECK (concurrency BETWEEN 1 AND 100),
    CONSTRAINT workflow_cache_iteration_batch_progress CHECK (next_batch_index = dispatched_count),
    CONSTRAINT workflow_cache_iteration_scan_progress CHECK (dispatched_count <= scanned_count),
    CONSTRAINT workflow_cache_iteration_error_summary_length
        CHECK (error_summary IS NULL OR OCTET_LENGTH(error_summary) <= 4096),
    CONSTRAINT workflow_cache_iteration_terminal_fields CHECK (
        (state = 'scanning' AND completed_at IS NULL AND error_summary IS NULL)
        OR (state = 'completed' AND completed_at IS NOT NULL AND error_summary IS NULL)
        OR (state IN ('failed', 'cancelled') AND completed_at IS NOT NULL)
    )
);

CREATE INDEX workflow_cache_iteration_generation_pin_idx
    ON workflow_cache_iteration (generation)
    WHERE state = 'scanning';

CREATE TRIGGER update_workflow_cache_iteration_updated
    BEFORE UPDATE ON workflow_cache_iteration
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE workflow_cache_iteration IS
    'Durable scan cursor and cache-generation retention pin for one workflow task';
COMMENT ON COLUMN workflow_cache_iteration.last_external_id IS
    'Last cache external ID consumed in bytewise C collation order';
COMMENT ON COLUMN workflow_cache_iteration.completed_at IS
    'Time the iteration entered any terminal state';
