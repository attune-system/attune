-- Migration: TimescaleDB Entity History and Analytics
-- Description: Creates append-only history hypertables for execution and worker tables.
--              Uses JSONB diff format to track field-level changes via PostgreSQL triggers.
--              Converts the event, enforcement, and execution tables into TimescaleDB
--              hypertables (events are immutable; enforcements are updated exactly once;
--              executions are updated ~4 times during their lifecycle).
--              Includes continuous aggregates for dashboard analytics.
--              See docs/plans/timescaledb-entity-history.md for full design.
--
--              NOTE: FK constraints that would reference hypertable targets were never
--              created in earlier migrations (000004, 000005, 000006), so no DROP
--              CONSTRAINT statements are needed here.
-- Version: 20250101000009

-- Set search_path for schema isolation
SET search_path TO attune, public;

-- ============================================================================
-- EXTENSION
-- ============================================================================

CREATE EXTENSION IF NOT EXISTS timescaledb;

-- ============================================================================
-- HELPER FUNCTIONS
-- ============================================================================

-- Returns a small {digest, size, type} object instead of the full JSONB value.
-- Used in history triggers for columns that can be arbitrarily large (e.g. result).
-- The full value is always available on the live row.
CREATE OR REPLACE FUNCTION _jsonb_digest_summary(val JSONB)
RETURNS JSONB AS $$
BEGIN
    IF val IS NULL THEN
        RETURN NULL;
    END IF;
    RETURN jsonb_build_object(
        'digest', 'md5:' || md5(val::text),
        'size',   octet_length(val::text),
        'type',   jsonb_typeof(val)
    );
END;
$$ LANGUAGE plpgsql IMMUTABLE;

COMMENT ON FUNCTION _jsonb_digest_summary(JSONB) IS
    'Returns a compact {digest, size, type} summary of a JSONB value for use in history tables. '
    'The digest is md5 of the text representation — sufficient for change-detection, not for security.';

-- ============================================================================
-- HISTORY TABLES
-- ============================================================================

-- ----------------------------------------------------------------------------
-- execution_history
-- ----------------------------------------------------------------------------

CREATE TABLE execution_history (
    time             TIMESTAMPTZ    NOT NULL DEFAULT NOW(),
    operation        TEXT           NOT NULL,
    entity_id        BIGINT         NOT NULL,
    entity_ref       TEXT,
    changed_fields   TEXT[]         NOT NULL DEFAULT '{}',
    old_values       JSONB,
    new_values       JSONB
);

SELECT create_hypertable('execution_history', 'time',
    chunk_time_interval => INTERVAL '1 day');

CREATE INDEX idx_execution_history_entity
    ON execution_history (entity_id, time DESC);

CREATE INDEX idx_execution_history_entity_ref
    ON execution_history (entity_ref, time DESC);

CREATE INDEX idx_execution_history_status_changes
    ON execution_history (time DESC)
    WHERE 'status' = ANY(changed_fields);

CREATE INDEX idx_execution_history_changed_fields
    ON execution_history USING GIN (changed_fields);

COMMENT ON TABLE execution_history IS 'Append-only history of field-level changes to the execution table (TimescaleDB hypertable)';
COMMENT ON COLUMN execution_history.time IS 'When the change occurred (hypertable partitioning dimension)';
COMMENT ON COLUMN execution_history.operation IS 'INSERT, UPDATE, or DELETE';
COMMENT ON COLUMN execution_history.entity_id IS 'execution.id of the changed row';
COMMENT ON COLUMN execution_history.entity_ref IS 'Denormalized action_ref for JOIN-free queries';
COMMENT ON COLUMN execution_history.changed_fields IS 'Array of field names that changed (empty for INSERT/DELETE)';
COMMENT ON COLUMN execution_history.old_values IS 'Previous values of changed fields (NULL for INSERT)';
COMMENT ON COLUMN execution_history.new_values IS 'New values of changed fields (NULL for DELETE)';

-- ----------------------------------------------------------------------------
-- worker_history
-- ----------------------------------------------------------------------------

CREATE TABLE worker_history (
    time             TIMESTAMPTZ    NOT NULL DEFAULT NOW(),
    operation        TEXT           NOT NULL,
    entity_id        BIGINT         NOT NULL,
    entity_ref       TEXT,
    changed_fields   TEXT[]         NOT NULL DEFAULT '{}',
    old_values       JSONB,
    new_values       JSONB
);

SELECT create_hypertable('worker_history', 'time',
    chunk_time_interval => INTERVAL '7 days');

CREATE INDEX idx_worker_history_entity
    ON worker_history (entity_id, time DESC);

CREATE INDEX idx_worker_history_entity_ref
    ON worker_history (entity_ref, time DESC);

CREATE INDEX idx_worker_history_status_changes
    ON worker_history (time DESC)
    WHERE 'status' = ANY(changed_fields);

CREATE INDEX idx_worker_history_changed_fields
    ON worker_history USING GIN (changed_fields);

COMMENT ON TABLE worker_history IS 'Append-only history of field-level changes to the worker table (TimescaleDB hypertable)';
COMMENT ON COLUMN worker_history.entity_ref IS 'Denormalized worker name for JOIN-free queries';

-- ----------------------------------------------------------------------------
-- sensor_process_history
-- ----------------------------------------------------------------------------

CREATE TABLE sensor_process_history (
    time             TIMESTAMPTZ    NOT NULL DEFAULT NOW(),
    operation        TEXT           NOT NULL,
    entity_id        BIGINT         NOT NULL,
    entity_ref       TEXT           NOT NULL,
    worker_id        BIGINT,
    worker_name      TEXT,
    changed_fields   TEXT[]         NOT NULL DEFAULT '{}',
    old_values       JSONB,
    new_values       JSONB
);

SELECT create_hypertable('sensor_process_history', 'time',
    chunk_time_interval => INTERVAL '7 days');

CREATE INDEX idx_sensor_process_history_entity
    ON sensor_process_history (entity_id, time DESC);
CREATE INDEX idx_sensor_process_history_entity_ref
    ON sensor_process_history (entity_ref, time DESC);
CREATE INDEX idx_sensor_process_history_worker
    ON sensor_process_history (worker_id, time DESC);
CREATE INDEX idx_sensor_process_history_status_changes
    ON sensor_process_history (time DESC)
    WHERE 'status' = ANY(changed_fields);
CREATE INDEX idx_sensor_process_history_changed_fields
    ON sensor_process_history USING GIN (changed_fields);

COMMENT ON TABLE sensor_process_history IS 'Append-only history of field-level changes to sensor_process live state';
COMMENT ON COLUMN sensor_process_history.entity_ref IS 'Denormalized sensor ref for JOIN-free queries';
COMMENT ON COLUMN sensor_process_history.worker_name IS 'Denormalized worker name for JOIN-free queries';

-- ============================================================================
-- CONVERT EVENT TABLE TO HYPERTABLE
-- ============================================================================
-- Events are immutable after insert — they are never updated. Instead of
-- maintaining a separate event_history table to track changes that never
-- happen, we convert the event table itself into a TimescaleDB hypertable
-- partitioned on `created`. This gives us automatic time-based partitioning,
-- compression, and retention for free.
--
-- No FK constraints reference event(id) — enforcement.event was created as a
-- plain BIGINT in migration 000004 (hypertables cannot be FK targets).
-- ----------------------------------------------------------------------------

-- Replace the single-column PK with a composite PK that includes the
-- partitioning column (required by TimescaleDB).
ALTER TABLE event DROP CONSTRAINT event_pkey;
ALTER TABLE event ADD PRIMARY KEY (id, created);

SELECT create_hypertable('event', 'created',
    chunk_time_interval => INTERVAL '1 day',
    migrate_data        => true);

COMMENT ON TABLE event IS 'Events are instances of triggers firing (TimescaleDB hypertable partitioned on created)';

COMMENT ON TABLE enforcement IS 'Enforcements represent rule triggering by events';
COMMENT ON TABLE execution IS 'Executions represent action runs with workflow support. History and analytics are stored in execution_history.';

-- ============================================================================
-- TRIGGER FUNCTIONS
-- ============================================================================

-- ----------------------------------------------------------------------------
-- execution history trigger
-- Tracked fields: status, result, executor, worker, workflow_task, env_vars, started_at
-- Note: result uses _jsonb_digest_summary() to avoid storing large payloads
-- ----------------------------------------------------------------------------

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
                    'started_at', NEW.started_at
                ));
        RETURN NEW;
    END IF;

    IF TG_OP = 'DELETE' THEN
        INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, old_values, new_values)
        VALUES (NOW(), 'DELETE', OLD.id, OLD.action_ref, '{}', NULL, NULL);
        RETURN OLD;
    END IF;

    -- UPDATE: detect which fields changed

    IF OLD.status IS DISTINCT FROM NEW.status THEN
        changed := array_append(changed, 'status');
        old_vals := old_vals || jsonb_build_object('status', OLD.status);
        new_vals := new_vals || jsonb_build_object('status', NEW.status);
    END IF;

    -- Result: store a compact digest instead of the full JSONB to avoid bloat.
    -- The live execution row always has the complete result.
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
        old_vals := old_vals || jsonb_build_object('env_vars', OLD.env_vars);
        new_vals := new_vals || jsonb_build_object('env_vars', NEW.env_vars);
    END IF;

    IF OLD.started_at IS DISTINCT FROM NEW.started_at THEN
        changed := array_append(changed, 'started_at');
        old_vals := old_vals || jsonb_build_object('started_at', OLD.started_at);
        new_vals := new_vals || jsonb_build_object('started_at', NEW.started_at);
    END IF;

    -- Only record if something actually changed
    IF array_length(changed, 1) > 0 THEN
        INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, old_values, new_values)
        VALUES (NOW(), 'UPDATE', NEW.id, NEW.action_ref, changed, old_vals, new_vals);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION record_execution_history() IS 'Records field-level changes to execution table in execution_history hypertable';

-- ----------------------------------------------------------------------------
-- worker history trigger
-- Tracked fields: name, status, capabilities, meta, host, port, cordon state
-- Excludes: last_heartbeat when it is the only field that changed
-- ----------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION record_worker_history()
RETURNS TRIGGER AS $$
DECLARE
    changed TEXT[] := '{}';
    old_vals JSONB := '{}';
    new_vals JSONB := '{}';
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO worker_history (time, operation, entity_id, entity_ref, changed_fields, old_values, new_values)
        VALUES (NOW(), 'INSERT', NEW.id, NEW.name, '{}', NULL,
                jsonb_build_object(
                    'name', NEW.name,
                    'worker_type', NEW.worker_type,
                    'worker_role', NEW.worker_role,
                    'status', NEW.status,
                    'capabilities', NEW.capabilities,
                    'meta', NEW.meta,
                    'host', NEW.host,
                    'port', NEW.port,
                    'cordoned', NEW.cordoned,
                    'cordon_reason', NEW.cordon_reason,
                    'cordoned_by', NEW.cordoned_by,
                    'cordoned_at', NEW.cordoned_at
                ));
        RETURN NEW;
    END IF;

    IF TG_OP = 'DELETE' THEN
        INSERT INTO worker_history (time, operation, entity_id, entity_ref, changed_fields, old_values, new_values)
        VALUES (NOW(), 'DELETE', OLD.id, OLD.name, '{}', to_jsonb(OLD), NULL);
        RETURN OLD;
    END IF;

    -- UPDATE: detect which fields changed
    IF OLD.name IS DISTINCT FROM NEW.name THEN
        changed := array_append(changed, 'name');
        old_vals := old_vals || jsonb_build_object('name', OLD.name);
        new_vals := new_vals || jsonb_build_object('name', NEW.name);
    END IF;

    IF OLD.status IS DISTINCT FROM NEW.status THEN
        changed := array_append(changed, 'status');
        old_vals := old_vals || jsonb_build_object('status', OLD.status);
        new_vals := new_vals || jsonb_build_object('status', NEW.status);
    END IF;

    IF OLD.capabilities IS DISTINCT FROM NEW.capabilities THEN
        changed := array_append(changed, 'capabilities');
        old_vals := old_vals || jsonb_build_object('capabilities', OLD.capabilities);
        new_vals := new_vals || jsonb_build_object('capabilities', NEW.capabilities);
    END IF;

    IF OLD.meta IS DISTINCT FROM NEW.meta THEN
        changed := array_append(changed, 'meta');
        old_vals := old_vals || jsonb_build_object('meta', OLD.meta);
        new_vals := new_vals || jsonb_build_object('meta', NEW.meta);
    END IF;

    IF OLD.host IS DISTINCT FROM NEW.host THEN
        changed := array_append(changed, 'host');
        old_vals := old_vals || jsonb_build_object('host', OLD.host);
        new_vals := new_vals || jsonb_build_object('host', NEW.host);
    END IF;

    IF OLD.port IS DISTINCT FROM NEW.port THEN
        changed := array_append(changed, 'port');
        old_vals := old_vals || jsonb_build_object('port', OLD.port);
        new_vals := new_vals || jsonb_build_object('port', NEW.port);
    END IF;

    IF OLD.cordoned IS DISTINCT FROM NEW.cordoned THEN
        changed := array_append(changed, 'cordoned');
        old_vals := old_vals || jsonb_build_object('cordoned', OLD.cordoned);
        new_vals := new_vals || jsonb_build_object('cordoned', NEW.cordoned);
    END IF;

    IF OLD.cordon_reason IS DISTINCT FROM NEW.cordon_reason THEN
        changed := array_append(changed, 'cordon_reason');
        old_vals := old_vals || jsonb_build_object('cordon_reason', OLD.cordon_reason);
        new_vals := new_vals || jsonb_build_object('cordon_reason', NEW.cordon_reason);
    END IF;

    IF OLD.cordoned_by IS DISTINCT FROM NEW.cordoned_by THEN
        changed := array_append(changed, 'cordoned_by');
        old_vals := old_vals || jsonb_build_object('cordoned_by', OLD.cordoned_by);
        new_vals := new_vals || jsonb_build_object('cordoned_by', NEW.cordoned_by);
    END IF;

    IF OLD.cordoned_at IS DISTINCT FROM NEW.cordoned_at THEN
        changed := array_append(changed, 'cordoned_at');
        old_vals := old_vals || jsonb_build_object('cordoned_at', OLD.cordoned_at);
        new_vals := new_vals || jsonb_build_object('cordoned_at', NEW.cordoned_at);
    END IF;

    -- Only record if something besides last_heartbeat changed.
    -- Pure heartbeat-only updates are excluded to avoid high-volume noise.
    IF array_length(changed, 1) > 0 THEN
        INSERT INTO worker_history (time, operation, entity_id, entity_ref, changed_fields, old_values, new_values)
        VALUES (NOW(), 'UPDATE', NEW.id, NEW.name, changed, old_vals, new_vals);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION record_worker_history() IS 'Records field-level changes to worker table in worker_history hypertable. Excludes heartbeat-only updates.';

-- ----------------------------------------------------------------------------
-- sensor process history trigger
-- ----------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION record_sensor_process_history()
RETURNS TRIGGER AS $$
DECLARE
    changed TEXT[] := '{}';
    old_vals JSONB := '{}';
    new_vals JSONB := '{}';
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO sensor_process_history (
            time, operation, entity_id, entity_ref, worker_id, worker_name,
            changed_fields, old_values, new_values
        )
        VALUES (
            NEW.created,
            'INSERT',
            NEW.id,
            NEW.sensor_ref,
            NEW.worker,
            NEW.worker_name,
            '{}',
            NULL,
            jsonb_build_object(
                'sensor', NEW.sensor,
                'sensor_ref', NEW.sensor_ref,
                'worker', NEW.worker,
                'worker_name', NEW.worker_name,
                'status', NEW.status,
                'pid', NEW.pid,
                'consecutive_failures', NEW.consecutive_failures,
                'last_exit_code', NEW.last_exit_code,
                'last_signal', NEW.last_signal,
                'last_started_at', NEW.last_started_at,
                'last_stopped_at', NEW.last_stopped_at,
                'next_restart_at', NEW.next_restart_at,
                'stderr_excerpt', NEW.stderr_excerpt,
                'log_artifact_ref', NEW.log_artifact_ref,
                'active_rule_count', NEW.active_rule_count,
                'last_alerted_failure_count', NEW.last_alerted_failure_count,
                'last_alerted_at', NEW.last_alerted_at,
                'meta', NEW.meta
            )
        );
        RETURN NEW;
    END IF;

    IF TG_OP = 'DELETE' THEN
        INSERT INTO sensor_process_history (
            time, operation, entity_id, entity_ref, worker_id, worker_name,
            changed_fields, old_values, new_values
        )
        VALUES (
            NOW(),
            'DELETE',
            OLD.id,
            OLD.sensor_ref,
            OLD.worker,
            OLD.worker_name,
            '{}',
            to_jsonb(OLD),
            NULL
        );
        RETURN OLD;
    END IF;

    IF OLD.sensor_ref IS DISTINCT FROM NEW.sensor_ref THEN
        changed := array_append(changed, 'sensor_ref');
        old_vals := old_vals || jsonb_build_object('sensor_ref', OLD.sensor_ref);
        new_vals := new_vals || jsonb_build_object('sensor_ref', NEW.sensor_ref);
    END IF;

    IF OLD.worker_name IS DISTINCT FROM NEW.worker_name THEN
        changed := array_append(changed, 'worker_name');
        old_vals := old_vals || jsonb_build_object('worker_name', OLD.worker_name);
        new_vals := new_vals || jsonb_build_object('worker_name', NEW.worker_name);
    END IF;

    IF OLD.status IS DISTINCT FROM NEW.status THEN
        changed := array_append(changed, 'status');
        old_vals := old_vals || jsonb_build_object('status', OLD.status);
        new_vals := new_vals || jsonb_build_object('status', NEW.status);
    END IF;

    IF OLD.pid IS DISTINCT FROM NEW.pid THEN
        changed := array_append(changed, 'pid');
        old_vals := old_vals || jsonb_build_object('pid', OLD.pid);
        new_vals := new_vals || jsonb_build_object('pid', NEW.pid);
    END IF;

    IF OLD.consecutive_failures IS DISTINCT FROM NEW.consecutive_failures THEN
        changed := array_append(changed, 'consecutive_failures');
        old_vals := old_vals || jsonb_build_object('consecutive_failures', OLD.consecutive_failures);
        new_vals := new_vals || jsonb_build_object('consecutive_failures', NEW.consecutive_failures);
    END IF;

    IF OLD.last_exit_code IS DISTINCT FROM NEW.last_exit_code THEN
        changed := array_append(changed, 'last_exit_code');
        old_vals := old_vals || jsonb_build_object('last_exit_code', OLD.last_exit_code);
        new_vals := new_vals || jsonb_build_object('last_exit_code', NEW.last_exit_code);
    END IF;

    IF OLD.last_signal IS DISTINCT FROM NEW.last_signal THEN
        changed := array_append(changed, 'last_signal');
        old_vals := old_vals || jsonb_build_object('last_signal', OLD.last_signal);
        new_vals := new_vals || jsonb_build_object('last_signal', NEW.last_signal);
    END IF;

    IF OLD.last_started_at IS DISTINCT FROM NEW.last_started_at THEN
        changed := array_append(changed, 'last_started_at');
        old_vals := old_vals || jsonb_build_object('last_started_at', OLD.last_started_at);
        new_vals := new_vals || jsonb_build_object('last_started_at', NEW.last_started_at);
    END IF;

    IF OLD.last_stopped_at IS DISTINCT FROM NEW.last_stopped_at THEN
        changed := array_append(changed, 'last_stopped_at');
        old_vals := old_vals || jsonb_build_object('last_stopped_at', OLD.last_stopped_at);
        new_vals := new_vals || jsonb_build_object('last_stopped_at', NEW.last_stopped_at);
    END IF;

    IF OLD.next_restart_at IS DISTINCT FROM NEW.next_restart_at THEN
        changed := array_append(changed, 'next_restart_at');
        old_vals := old_vals || jsonb_build_object('next_restart_at', OLD.next_restart_at);
        new_vals := new_vals || jsonb_build_object('next_restart_at', NEW.next_restart_at);
    END IF;

    IF OLD.stderr_excerpt IS DISTINCT FROM NEW.stderr_excerpt THEN
        changed := array_append(changed, 'stderr_excerpt');
        old_vals := old_vals || jsonb_build_object('stderr_excerpt', OLD.stderr_excerpt);
        new_vals := new_vals || jsonb_build_object('stderr_excerpt', NEW.stderr_excerpt);
    END IF;

    IF OLD.log_artifact_ref IS DISTINCT FROM NEW.log_artifact_ref THEN
        changed := array_append(changed, 'log_artifact_ref');
        old_vals := old_vals || jsonb_build_object('log_artifact_ref', OLD.log_artifact_ref);
        new_vals := new_vals || jsonb_build_object('log_artifact_ref', NEW.log_artifact_ref);
    END IF;

    IF OLD.active_rule_count IS DISTINCT FROM NEW.active_rule_count THEN
        changed := array_append(changed, 'active_rule_count');
        old_vals := old_vals || jsonb_build_object('active_rule_count', OLD.active_rule_count);
        new_vals := new_vals || jsonb_build_object('active_rule_count', NEW.active_rule_count);
    END IF;

    IF OLD.last_alerted_failure_count IS DISTINCT FROM NEW.last_alerted_failure_count THEN
        changed := array_append(changed, 'last_alerted_failure_count');
        old_vals := old_vals || jsonb_build_object('last_alerted_failure_count', OLD.last_alerted_failure_count);
        new_vals := new_vals || jsonb_build_object('last_alerted_failure_count', NEW.last_alerted_failure_count);
    END IF;

    IF OLD.last_alerted_at IS DISTINCT FROM NEW.last_alerted_at THEN
        changed := array_append(changed, 'last_alerted_at');
        old_vals := old_vals || jsonb_build_object('last_alerted_at', OLD.last_alerted_at);
        new_vals := new_vals || jsonb_build_object('last_alerted_at', NEW.last_alerted_at);
    END IF;

    IF OLD.meta IS DISTINCT FROM NEW.meta THEN
        changed := array_append(changed, 'meta');
        old_vals := old_vals || jsonb_build_object('meta', OLD.meta);
        new_vals := new_vals || jsonb_build_object('meta', NEW.meta);
    END IF;

    IF array_length(changed, 1) > 0 THEN
        INSERT INTO sensor_process_history (
            time, operation, entity_id, entity_ref, worker_id, worker_name,
            changed_fields, old_values, new_values
        )
        VALUES (
            NEW.updated,
            'UPDATE',
            NEW.id,
            NEW.sensor_ref,
            NEW.worker,
            NEW.worker_name,
            changed,
            old_vals,
            new_vals
        );
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION record_sensor_process_history() IS 'Records field-level changes to sensor_process in sensor_process_history hypertable';

-- ============================================================================
-- ATTACH TRIGGERS TO OPERATIONAL TABLES
-- ============================================================================

CREATE TRIGGER execution_history_trigger
    AFTER INSERT OR UPDATE OR DELETE ON execution
    FOR EACH ROW
    EXECUTE FUNCTION record_execution_history();

CREATE TRIGGER worker_history_trigger
    AFTER INSERT OR UPDATE OR DELETE ON worker
    FOR EACH ROW
    EXECUTE FUNCTION record_worker_history();

CREATE TRIGGER sensor_process_history_trigger
    AFTER INSERT OR UPDATE OR DELETE ON sensor_process
    FOR EACH ROW
    EXECUTE FUNCTION record_sensor_process_history();

-- ============================================================================
-- COMPRESSION POLICIES
-- ============================================================================
-- Schema-per-test databases need the Timescale objects but must not register
-- database-global background jobs, which can outlive their temporary schemas.

-- History tables
ALTER TABLE execution_history SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'entity_id',
    timescaledb.compress_orderby = 'time DESC'
);
SELECT add_compression_policy('execution_history', INTERVAL '7 days')
WHERE current_schema() NOT LIKE 'test\_%' ESCAPE '\';

ALTER TABLE worker_history SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'entity_id',
    timescaledb.compress_orderby = 'time DESC'
);
SELECT add_compression_policy('worker_history', INTERVAL '7 days')
WHERE current_schema() NOT LIKE 'test\_%' ESCAPE '\';

ALTER TABLE sensor_process_history SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'entity_id',
    timescaledb.compress_orderby = 'time DESC'
);
SELECT add_compression_policy('sensor_process_history', INTERVAL '7 days')
WHERE current_schema() NOT LIKE 'test\_%' ESCAPE '\';

-- Event table (hypertable)
ALTER TABLE event SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'trigger_ref',
    timescaledb.compress_orderby = 'created DESC, id DESC'
);
SELECT add_compression_policy('event', INTERVAL '7 days')
WHERE current_schema() NOT LIKE 'test\_%' ESCAPE '\';

-- ============================================================================
-- CONTINUOUS AGGREGATES
-- ============================================================================

-- Drop existing continuous aggregates if they exist, so this migration can be
-- re-run safely after a partial failure. (TimescaleDB continuous aggregates
-- must be dropped with CASCADE to remove their associated policies.)
-- Try DROP VIEW first (handles the case where an earlier run created a plain
-- view instead of a materialized view), then DROP MATERIALIZED VIEW.
DROP VIEW IF EXISTS execution_status_hourly CASCADE;
DROP MATERIALIZED VIEW IF EXISTS execution_status_hourly CASCADE;
DROP VIEW IF EXISTS execution_throughput_hourly CASCADE;
DROP MATERIALIZED VIEW IF EXISTS execution_throughput_hourly CASCADE;
DROP VIEW IF EXISTS event_volume_hourly CASCADE;
DROP MATERIALIZED VIEW IF EXISTS event_volume_hourly CASCADE;
DROP VIEW IF EXISTS worker_status_hourly CASCADE;
DROP MATERIALIZED VIEW IF EXISTS worker_status_hourly CASCADE;
DROP VIEW IF EXISTS enforcement_volume_hourly CASCADE;
DROP MATERIALIZED VIEW IF EXISTS enforcement_volume_hourly CASCADE;
DROP VIEW IF EXISTS execution_volume_hourly CASCADE;
DROP MATERIALIZED VIEW IF EXISTS execution_volume_hourly CASCADE;

-- ----------------------------------------------------------------------------
-- execution_status_hourly
-- Tracks execution status transitions per hour, grouped by action_ref and new status.
-- Powers: execution throughput chart, failure rate widget, status breakdown over time.
-- ----------------------------------------------------------------------------

CREATE MATERIALIZED VIEW execution_status_hourly
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', time) AS bucket,
    entity_ref AS action_ref,
    new_values->>'status' AS new_status,
    COUNT(*) AS transition_count
FROM execution_history
WHERE 'status' = ANY(changed_fields)
GROUP BY bucket, entity_ref, new_values->>'status'
WITH NO DATA;

SELECT add_continuous_aggregate_policy('execution_status_hourly',
    start_offset    => INTERVAL '7 days',
    end_offset      => INTERVAL '1 hour',
    schedule_interval => INTERVAL '30 minutes'
)
WHERE current_schema() NOT LIKE 'test\_%' ESCAPE '\';

-- ----------------------------------------------------------------------------
-- execution_throughput_hourly
-- Tracks total execution creation volume per hour, regardless of status.
-- Powers: execution throughput sparkline on the dashboard.
-- ----------------------------------------------------------------------------

CREATE MATERIALIZED VIEW execution_throughput_hourly
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', time) AS bucket,
    entity_ref AS action_ref,
    COUNT(*) AS execution_count
FROM execution_history
WHERE operation = 'INSERT'
GROUP BY bucket, entity_ref
WITH NO DATA;

SELECT add_continuous_aggregate_policy('execution_throughput_hourly',
    start_offset    => INTERVAL '7 days',
    end_offset      => INTERVAL '1 hour',
    schedule_interval => INTERVAL '30 minutes'
)
WHERE current_schema() NOT LIKE 'test\_%' ESCAPE '\';

-- ----------------------------------------------------------------------------
-- event_volume_hourly
-- Tracks event creation volume per hour by trigger ref.
-- Powers: event throughput monitoring widget.
-- NOTE: Queries the event table directly (it is now a hypertable) instead of
--       a separate event_history table.
-- ----------------------------------------------------------------------------

CREATE MATERIALIZED VIEW event_volume_hourly
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', created) AS bucket,
    trigger_ref,
    COUNT(*) AS event_count
FROM event
GROUP BY bucket, trigger_ref
WITH NO DATA;

SELECT add_continuous_aggregate_policy('event_volume_hourly',
    start_offset    => INTERVAL '7 days',
    end_offset      => INTERVAL '1 hour',
    schedule_interval => INTERVAL '30 minutes'
)
WHERE current_schema() NOT LIKE 'test\_%' ESCAPE '\';

-- ----------------------------------------------------------------------------
-- worker_status_hourly
-- Tracks worker status changes per hour (online/offline/draining transitions).
-- Powers: worker health trends widget.
-- ----------------------------------------------------------------------------

CREATE MATERIALIZED VIEW worker_status_hourly
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', time) AS bucket,
    entity_ref AS worker_name,
    new_values->>'status' AS new_status,
    COUNT(*) AS transition_count
FROM worker_history
WHERE 'status' = ANY(changed_fields)
GROUP BY bucket, entity_ref, new_values->>'status'
WITH NO DATA;

SELECT add_continuous_aggregate_policy('worker_status_hourly',
    start_offset    => INTERVAL '30 days',
    end_offset      => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour'
)
WHERE current_schema() NOT LIKE 'test\_%' ESCAPE '\';

-- ----------------------------------------------------------------------------
-- enforcement_volume_hourly
-- Tracks enforcement creation volume per hour by rule ref.
-- Powers: rule activation rate monitoring.
-- NOTE: Queries the enforcement table directly (it is now a hypertable)
--       instead of a separate enforcement_history table.
-- ----------------------------------------------------------------------------

CREATE VIEW enforcement_volume_hourly AS
SELECT
    date_trunc('hour', created) AS bucket,
    rule_ref,
    COUNT(*) AS enforcement_count
FROM enforcement
GROUP BY bucket, rule_ref
;

-- ----------------------------------------------------------------------------
-- execution_volume_hourly
-- Tracks execution creation volume per hour by action_ref and status.
-- This queries the execution table directly. Complements the existing
-- execution_status_hourly and execution_throughput_hourly aggregates which
-- query execution_history.
--
-- Use case: direct execution volume monitoring without relying on the history
-- trigger (belt-and-suspenders, plus captures the initial status at creation).
-- ----------------------------------------------------------------------------

CREATE VIEW execution_volume_hourly AS
SELECT
    date_trunc('hour', created) AS bucket,
    action_ref,
    status AS initial_status,
    COUNT(*) AS execution_count
FROM execution
GROUP BY bucket, action_ref, status
;

-- ============================================================================
-- INITIAL REFRESH NOTE
-- ============================================================================
-- NOTE: refresh_continuous_aggregate() cannot run inside a transaction block,
-- and the migration runner wraps each file in BEGIN/COMMIT. The continuous
-- aggregate policies configured above will automatically backfill data within
-- their first scheduled interval (30 min – 1 hour). On a fresh database there
-- is no history data to backfill anyway.
--
-- If you need an immediate manual refresh after migration, run outside a
-- transaction:
--   CALL refresh_continuous_aggregate('execution_status_hourly', NULL, NOW());
--   CALL refresh_continuous_aggregate('execution_throughput_hourly', NULL, NOW());
--   CALL refresh_continuous_aggregate('event_volume_hourly', NULL, NOW());
--   CALL refresh_continuous_aggregate('worker_status_hourly', NULL, NOW());
--   CALL refresh_continuous_aggregate('enforcement_volume_hourly', NULL, NOW());
--   CALL refresh_continuous_aggregate('execution_volume_hourly', NULL, NOW());
