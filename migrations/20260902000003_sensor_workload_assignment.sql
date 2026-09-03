-- Add durable sensor workloads and fenced worker assignments.

SET search_path TO attune, public;

CREATE TABLE sensor_workload (
    id BIGSERIAL PRIMARY KEY,
    sensor BIGINT NOT NULL REFERENCES sensor(id) ON DELETE CASCADE,
    workload_key TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT sensor_workload_key_nonempty CHECK (BTRIM(workload_key) <> ''),
    CONSTRAINT uq_sensor_workload_sensor_key UNIQUE (sensor, workload_key)
);

CREATE INDEX idx_sensor_workload_sensor ON sensor_workload(sensor);

CREATE TRIGGER update_sensor_workload_updated
    BEFORE UPDATE ON sensor_workload
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

CREATE TABLE sensor_workload_assignment (
    workload BIGINT PRIMARY KEY REFERENCES sensor_workload(id) ON DELETE CASCADE,
    worker BIGINT REFERENCES worker(id) ON DELETE RESTRICT,
    worker_instance UUID,
    generation BIGINT NOT NULL DEFAULT 0,
    lease_expires_at TIMESTAMPTZ,
    assigned_at TIMESTAMPTZ,
    renewed_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT sensor_workload_assignment_generation_nonnegative CHECK (generation >= 0),
    CONSTRAINT sensor_workload_assignment_owner_complete CHECK (
        (
            worker IS NULL
            AND worker_instance IS NULL
            AND lease_expires_at IS NULL
            AND assigned_at IS NULL
            AND renewed_at IS NULL
        )
        OR
        (
            worker IS NOT NULL
            AND worker_instance IS NOT NULL
            AND lease_expires_at IS NOT NULL
            AND assigned_at IS NOT NULL
            AND renewed_at IS NOT NULL
        )
    )
);

CREATE INDEX idx_sensor_workload_assignment_worker
    ON sensor_workload_assignment(worker)
    WHERE worker IS NOT NULL;

CREATE INDEX idx_sensor_workload_assignment_lease
    ON sensor_workload_assignment(lease_expires_at)
    WHERE worker IS NOT NULL;

CREATE TRIGGER update_sensor_workload_assignment_updated
    BEFORE UPDATE ON sensor_workload_assignment
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

ALTER TABLE rule
    ADD COLUMN sensor_worker_selector JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN sensor_worker_tolerations JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN sensor_worker_affinity JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD CONSTRAINT rule_sensor_worker_selector_object
        CHECK (jsonb_typeof(sensor_worker_selector) = 'object'),
    ADD CONSTRAINT rule_sensor_worker_tolerations_array
        CHECK (jsonb_typeof(sensor_worker_tolerations) = 'array'),
    ADD CONSTRAINT rule_sensor_worker_affinity_object
        CHECK (jsonb_typeof(sensor_worker_affinity) = 'object');
