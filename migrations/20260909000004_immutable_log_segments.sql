CREATE TABLE log_stream (
    id BIGSERIAL PRIMARY KEY,
    artifact_version BIGINT NOT NULL UNIQUE REFERENCES artifact_version(id) ON DELETE CASCADE,
    max_unflushed_bytes BIGINT NOT NULL CHECK (max_unflushed_bytes > 0),
    max_unflushed_milliseconds BIGINT NOT NULL CHECK (max_unflushed_milliseconds > 0),
    next_sequence BIGINT NOT NULL DEFAULT 0 CHECK (next_sequence >= 0),
    total_bytes BIGINT NOT NULL DEFAULT 0 CHECK (total_bytes >= 0),
    truncated BOOLEAN NOT NULL DEFAULT FALSE,
    sealed BOOLEAN NOT NULL DEFAULT FALSE,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    sealed_at TIMESTAMPTZ
);

CREATE TABLE log_segment (
    id BIGSERIAL PRIMARY KEY,
    stream BIGINT NOT NULL REFERENCES log_stream(id) ON DELETE CASCADE,
    sequence BIGINT NOT NULL CHECK (sequence >= 0),
    byte_start BIGINT NOT NULL CHECK (byte_start >= 0),
    byte_end BIGINT NOT NULL CHECK (byte_end > byte_start),
    size_bytes BIGINT NOT NULL CHECK (size_bytes = byte_end - byte_start),
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    object_key TEXT NOT NULL UNIQUE,
    provider_version TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_log_segment_stream_sequence UNIQUE (stream, sequence)
);

COMMENT ON TABLE log_stream IS 'Ordered immutable byte stream backing one runtime-log artifact version.';
COMMENT ON COLUMN log_stream.max_unflushed_bytes IS 'Configured maximum bytes held by a producer before segment upload.';
COMMENT ON COLUMN log_stream.max_unflushed_milliseconds IS 'Configured maximum age of buffered bytes before segment upload.';
COMMENT ON TABLE log_segment IS 'Create-only object metadata for one contiguous log-stream byte range.';
