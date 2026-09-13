CREATE TABLE workflow_log_outbox (
    id BIGSERIAL PRIMARY KEY,
    workflow_execution BIGINT NOT NULL REFERENCES workflow_execution(id) ON DELETE CASCADE,
    sequence BIGINT NOT NULL CHECK (sequence >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('append', 'seal')),
    payload BYTEA,
    available_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    claimed_by UUID,
    claim_expires_at TIMESTAMPTZ,
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    last_error TEXT,
    delivered_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT workflow_log_outbox_stream_sequence_unique
        UNIQUE (workflow_execution, sequence),
    CONSTRAINT workflow_log_outbox_payload_shape CHECK (
        (kind = 'append' AND (
            (delivered_at IS NULL AND payload IS NOT NULL AND OCTET_LENGTH(payload) > 0)
            OR (delivered_at IS NOT NULL AND payload IS NULL)
        ))
        OR (kind = 'seal' AND payload IS NULL)
    ),
    CONSTRAINT workflow_log_outbox_claim_shape CHECK (
        (claimed_by IS NULL AND claim_expires_at IS NULL)
        OR (claimed_by IS NOT NULL AND claim_expires_at IS NOT NULL)
    )
);

CREATE UNIQUE INDEX workflow_log_outbox_one_seal_per_stream
    ON workflow_log_outbox (workflow_execution)
    WHERE kind = 'seal';

CREATE INDEX workflow_log_outbox_pending_idx
    ON workflow_log_outbox (available_at, workflow_execution, sequence)
    WHERE delivered_at IS NULL;

COMMENT ON TABLE workflow_log_outbox IS
    'Durable ordered workflow activity-log delivery queue processed by executor replicas.';
COMMENT ON COLUMN workflow_log_outbox.sequence IS
    'Monotonic per-workflow stream position. The terminal seal occupies the final position.';
COMMENT ON COLUMN workflow_log_outbox.claim_expires_at IS
    'Crash-recovery lease. Expired claims may be adopted by another executor replica.';
COMMENT ON COLUMN workflow_log_outbox.payload IS
    'Append bytes retained until delivery, then cleared while the sequence row remains.';
