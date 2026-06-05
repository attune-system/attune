-- Migration: Runtime Retention Supervisor
-- Description: Removes hard-coded TimescaleDB retention jobs so runtime row
--              retention is controlled by the configurable attune-supervisor
--              service. Compression policies remain in place.
-- Version: 20250101000014

SET search_path TO attune, public;

-- Existing development databases may already have retention jobs installed by
-- earlier migrations. The supervisor now owns retention windows and calls
-- drop_chunks with deployment-specific cutoffs, so remove static policies.
SELECT remove_retention_policy('execution_history', if_exists => true);
SELECT remove_retention_policy('worker_history', if_exists => true);
SELECT remove_retention_policy('sensor_process_history', if_exists => true);
SELECT remove_retention_policy('event', if_exists => true);
SELECT remove_retention_policy('audit_event', if_exists => true);

CREATE TABLE runtime_retention_config (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    check_interval_seconds BIGINT NOT NULL DEFAULT 3600,
    batch_size BIGINT NOT NULL DEFAULT 1000,
    dry_run BOOLEAN NOT NULL DEFAULT FALSE,
    advisory_lock_key BIGINT NOT NULL DEFAULT 7821001,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT runtime_retention_config_singleton CHECK (id = TRUE),
    CONSTRAINT runtime_retention_check_interval_positive CHECK (check_interval_seconds > 0),
    CONSTRAINT runtime_retention_batch_size_positive CHECK (batch_size > 0)
);

CREATE TRIGGER update_runtime_retention_config_updated
    BEFORE UPDATE ON runtime_retention_config
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

CREATE TABLE runtime_retention_target_config (
    target TEXT PRIMARY KEY,
    max_age_seconds BIGINT,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT runtime_retention_target_max_age_positive CHECK (
        max_age_seconds IS NULL OR max_age_seconds > 0
    )
);

CREATE TRIGGER update_runtime_retention_target_config_updated
    BEFORE UPDATE ON runtime_retention_target_config
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

CREATE TABLE supervisor_run (
    id TEXT PRIMARY KEY,
    service_name TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    stopped_at TIMESTAMPTZ,
    clean_shutdown BOOLEAN NOT NULL DEFAULT FALSE,
    stop_reason TEXT,
    meta JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX idx_supervisor_run_service_started
    ON supervisor_run (service_name, started_at DESC);

CREATE INDEX idx_supervisor_run_unclean
    ON supervisor_run (service_name, started_at DESC)
    WHERE clean_shutdown = FALSE AND stopped_at IS NULL;

CREATE TABLE execution_reschedule_state (
    execution_id BIGINT PRIMARY KEY,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TIMESTAMPTZ,
    last_reason TEXT,
    last_source TEXT,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT execution_reschedule_state_attempt_count_nonnegative CHECK (attempt_count >= 0)
);

CREATE INDEX idx_execution_reschedule_state_last_attempt
    ON execution_reschedule_state (last_attempt_at);

CREATE TRIGGER update_execution_reschedule_state_updated
    BEFORE UPDATE ON execution_reschedule_state
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

INSERT INTO runtime_retention_config (id, enabled, check_interval_seconds, batch_size, dry_run, advisory_lock_key)
VALUES (TRUE, TRUE, 3600, 1000, FALSE, 7821001)
ON CONFLICT (id) DO NOTHING;

INSERT INTO runtime_retention_target_config (target, max_age_seconds)
VALUES
    ('events', 2592000),
    ('enforcements', 2592000),
    ('executions', 2592000),
    ('execution_history', 2592000),
    ('worker_history', 2592000),
    ('sensor_process_history', 2592000),
    ('audit_events', 7776000),
    ('continuous_aggregates', 2592000),
    ('notifications', 2592000),
    ('webhook_event_logs', 2592000),
    ('inquiries', 2592000),
    ('work_queue_items', 2592000),
    ('work_queue_dispatches', 2592000),
    ('pack_test_executions', 2592000),
    ('execution_admission', 2592000),
    ('workers', 2592000),
    ('sensor_processes', 2592000)
ON CONFLICT (target) DO NOTHING;

COMMENT ON TABLE event IS
    'Events are instances of triggers firing (TimescaleDB hypertable partitioned on created; retention is managed by attune-supervisor).';
COMMENT ON TABLE audit_event IS
    'Security-grade audit trail (TimescaleDB hypertable partitioned on created; retention is managed by attune-supervisor).';
COMMENT ON TABLE execution_history IS
    'Append-only history of field-level changes to the execution table (TimescaleDB hypertable; retention is managed by attune-supervisor).';
COMMENT ON TABLE worker_history IS
    'Append-only history of field-level changes to the worker table (TimescaleDB hypertable; retention is managed by attune-supervisor).';
COMMENT ON TABLE sensor_process_history IS
    'Append-only history of field-level changes to sensor_process live state (TimescaleDB hypertable; retention is managed by attune-supervisor).';
COMMENT ON TABLE runtime_retention_config IS
    'Singleton runtime retention settings read by attune-supervisor each cycle and managed through the API.';
COMMENT ON TABLE runtime_retention_target_config IS
    'Per-target runtime retention settings read by attune-supervisor each cycle and managed through the API.';
COMMENT ON TABLE supervisor_run IS
    'Supervisor process lifecycle markers used to detect prior dirty shutdowns and make startup recovery explicit.';
COMMENT ON TABLE execution_reschedule_state IS
    'Operational bookkeeping for bounded republish attempts of stuck requested executions.';
