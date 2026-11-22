-- A manager sees only the key of the version it is authorized to produce.
-- Retain the binding with the artifact so later readers can derive that key.
CREATE TABLE manager_log_scope (
    version BIGINT PRIMARY KEY REFERENCES artifact_version(id) ON DELETE CASCADE,
    assignment BIGINT NOT NULL,
    worker BIGINT NOT NULL,
    pool TEXT NOT NULL,
    slot TEXT NOT NULL,
    stream_name TEXT NOT NULL CHECK (stream_name IN ('stdout','stderr')),
    UNIQUE (assignment, stream_name)
);
