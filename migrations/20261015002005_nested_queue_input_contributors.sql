-- Preserve each contributor through execution -> item -> execution chains.
-- Contributor keys identify snapshots, never a flattened union of grants.
ALTER TABLE recorded_queue_execution_input ADD COLUMN contributor TEXT NOT NULL DEFAULT '';
ALTER TABLE recorded_queue_execution_input DROP CONSTRAINT recorded_queue_execution_input_pkey;
ALTER TABLE recorded_queue_execution_input ADD PRIMARY KEY (execution,item,contributor);
ALTER TABLE recorded_queue_execution_input ADD CHECK (expected_items <= 256);

CREATE TABLE recorded_queue_item_input (
    item BIGINT NOT NULL REFERENCES work_queue_item(id) ON DELETE CASCADE,
    contributor TEXT NOT NULL CHECK (length(contributor) BETWEEN 1 AND 4096),
    expected_contributors INTEGER NOT NULL CHECK (expected_contributors BETWEEN 1 AND 256),
    authority JSONB NOT NULL,
    authority_digest TEXT NOT NULL CHECK (authority_digest ~ '^[0-9a-f]{64}$'),
    PRIMARY KEY (item,contributor)
);
CREATE TRIGGER guard_recorded_queue_item_input BEFORE UPDATE ON recorded_queue_item_input
    FOR EACH ROW EXECUTE FUNCTION guard_recorded_execution_binding();
