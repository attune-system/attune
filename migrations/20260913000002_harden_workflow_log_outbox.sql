ALTER TABLE workflow_log_outbox
    DROP CONSTRAINT workflow_log_outbox_workflow_execution_fkey,
    ADD CONSTRAINT workflow_log_outbox_workflow_execution_fkey
        FOREIGN KEY (workflow_execution) REFERENCES workflow_execution(id) ON DELETE RESTRICT,
    ADD COLUMN delivery_sequence BIGINT CHECK (delivery_sequence >= 0),
    ADD COLUMN failed_at TIMESTAMPTZ,
    ADD COLUMN is_head BOOLEAN NOT NULL DEFAULT FALSE,
    ADD CONSTRAINT workflow_log_outbox_terminal_shape CHECK (
        NOT (delivered_at IS NOT NULL AND failed_at IS NOT NULL)
        AND NOT (delivered_at IS NOT NULL AND is_head)
        AND (failed_at IS NULL OR is_head)
        AND (failed_at IS NULL OR (
            delivered_at IS NULL
            AND claimed_by IS NULL
            AND claim_expires_at IS NULL
            AND last_error IS NOT NULL
        ))
    );

UPDATE workflow_log_outbox outbox
SET is_head = TRUE
WHERE outbox.id IN (
    SELECT DISTINCT ON (workflow_execution) id
    FROM workflow_log_outbox
    WHERE delivered_at IS NULL
    ORDER BY workflow_execution, sequence
);

CREATE UNIQUE INDEX workflow_log_outbox_one_head_per_stream
    ON workflow_log_outbox (workflow_execution)
    WHERE is_head;

CREATE UNIQUE INDEX workflow_log_outbox_delivery_sequence_unique
    ON workflow_log_outbox (workflow_execution, delivery_sequence)
    WHERE delivery_sequence IS NOT NULL;

DROP INDEX workflow_log_outbox_pending_idx;

CREATE INDEX workflow_log_outbox_stream_head_idx
    ON workflow_log_outbox (workflow_execution, sequence)
    INCLUDE (id, available_at, claim_expires_at, failed_at)
    WHERE delivered_at IS NULL;

CREATE INDEX workflow_log_outbox_claimable_head_idx
    ON workflow_log_outbox (available_at, created, id)
    WHERE is_head AND delivered_at IS NULL AND failed_at IS NULL;

CREATE FUNCTION delete_delivered_workflow_log_outbox() RETURNS TRIGGER AS $$
BEGIN
    DELETE FROM workflow_log_outbox
    WHERE workflow_execution = OLD.id AND delivered_at IS NOT NULL;
    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER workflow_execution_delete_delivered_log_outbox
    BEFORE DELETE ON workflow_execution
    FOR EACH ROW EXECUTE FUNCTION delete_delivered_workflow_log_outbox();

COMMENT ON COLUMN workflow_log_outbox.delivery_sequence IS
    'Persisted log-stream sequence assigned before transport I/O for replay-safe delivery.';
COMMENT ON COLUMN workflow_log_outbox.failed_at IS
    'Permanent delivery failure. The row remains the stream head until explicitly retried.';
