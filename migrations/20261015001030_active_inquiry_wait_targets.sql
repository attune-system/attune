-- Historical inquiry identity survives independent raw inquiry retention.
-- Only a waiting dependency owns the physical target. Keep the wait itself as
-- the workflow's scheduler/idempotency fence after release.
ALTER TABLE workflow_task_wait ADD COLUMN active_inquiry BIGINT;

UPDATE workflow_task_wait SET active_inquiry = inquiry
WHERE kind = 'inquiry' AND state = 'waiting';

ALTER TABLE workflow_task_wait
    ADD CONSTRAINT workflow_task_wait_active_inquiry_fkey
        FOREIGN KEY (active_inquiry) REFERENCES inquiry(id) ON DELETE RESTRICT,
    ADD CONSTRAINT workflow_task_wait_active_inquiry_consistent CHECK (
        CASE WHEN kind = 'inquiry' AND state = 'waiting'
            THEN active_inquiry IS NOT NULL AND active_inquiry = inquiry
            ELSE active_inquiry IS NULL END
    );

-- Derive the dependency for every writer, including bounded tree cancellation
-- and older callers during rollout. Never permit a waiting historical identity
-- to shed its FK by writing NULL. The FK still arbitrates concurrent deletion.
CREATE FUNCTION workflow_task_wait_set_active_inquiry() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    NEW.active_inquiry := CASE
        WHEN NEW.kind = 'inquiry' AND NEW.state = 'waiting' THEN NEW.inquiry
        ELSE NULL
    END;
    RETURN NEW;
END;
$$;

CREATE TRIGGER workflow_task_wait_active_inquiry
BEFORE INSERT OR UPDATE ON workflow_task_wait
FOR EACH ROW EXECUTE FUNCTION workflow_task_wait_set_active_inquiry();

-- Validate/backfill the replacement before removing the historical FK. Any
-- missing waiting target aborts the migration rather than weakening safety.
ALTER TABLE workflow_task_wait DROP CONSTRAINT workflow_task_wait_inquiry_fkey;
CREATE INDEX workflow_task_wait_active_inquiry_idx
ON workflow_task_wait(active_inquiry) WHERE active_inquiry IS NOT NULL;
