SET search_path TO attune, public;

CREATE TABLE execution_log_stream_lease (
    id UUID PRIMARY KEY,
    identity_id BIGINT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    renewed TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

CREATE INDEX idx_execution_log_stream_lease_expires
    ON execution_log_stream_lease (expires_at);
CREATE INDEX idx_execution_log_stream_lease_identity_expires
    ON execution_log_stream_lease (identity_id, expires_at);
