CREATE TABLE recorded_execution_input_authority (
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('execution', 'queue_item')),
    owner_id BIGINT NOT NULL CHECK (owner_id > 0),
    authority JSONB NOT NULL CHECK (jsonb_typeof(authority) = 'object'),
    authority_digest TEXT NOT NULL CHECK (authority_digest ~ '^[0-9a-f]{64}$'),
    PRIMARY KEY (owner_kind, owner_id)
);
CREATE TRIGGER guard_recorded_execution_input_authority
    BEFORE UPDATE ON recorded_execution_input_authority
    FOR EACH ROW EXECUTE FUNCTION guard_recorded_execution_binding();
CREATE FUNCTION delete_recorded_execution_input_authority() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM recorded_execution_input_authority
        WHERE owner_kind = CASE WHEN TG_TABLE_NAME = 'execution' THEN 'execution' ELSE 'queue_item' END
        AND owner_id = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_execution_recorded_input_authority AFTER DELETE ON execution
    FOR EACH ROW EXECUTE FUNCTION delete_recorded_execution_input_authority();
CREATE TRIGGER delete_queue_item_recorded_input_authority AFTER DELETE ON work_queue_item
    FOR EACH ROW EXECUTE FUNCTION delete_recorded_execution_input_authority();

-- Copy accepted item authority into the execution, so item retention cannot
-- erase delivery evidence needed by a retained execution or its retries.
CREATE TABLE recorded_queue_execution_input (
    execution BIGINT NOT NULL REFERENCES execution(id) ON DELETE CASCADE,
    item BIGINT NOT NULL CHECK (item > 0),
    expected_items INTEGER NOT NULL CHECK (expected_items > 0),
    authority JSONB NOT NULL,
    authority_digest TEXT NOT NULL CHECK (authority_digest ~ '^[0-9a-f]{64}$'),
    PRIMARY KEY (execution, item)
);
CREATE TRIGGER guard_recorded_queue_execution_input BEFORE UPDATE ON recorded_queue_execution_input
    FOR EACH ROW EXECUTE FUNCTION guard_recorded_execution_binding();
