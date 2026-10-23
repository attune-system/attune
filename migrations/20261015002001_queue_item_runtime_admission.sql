-- Accepted queue work retains its invocation ceiling independently of humans.
CREATE TABLE work_queue_item_runtime_admission (
    item BIGINT PRIMARY KEY REFERENCES work_queue_item(id) ON DELETE CASCADE,
    policy_revision BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id),
    authority JSONB NOT NULL CHECK (jsonb_typeof(authority) = 'object'),
    authority_digest TEXT NOT NULL CHECK (authority_digest ~ '^[0-9a-f]{64}$')
);
CREATE INDEX work_queue_item_runtime_admission_policy_idx
    ON work_queue_item_runtime_admission(policy_revision);
CREATE FUNCTION guard_recorded_execution_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Recorded execution and input bindings are immutable';
END $$;
CREATE TRIGGER guard_work_queue_item_runtime_admission
    BEFORE UPDATE ON work_queue_item_runtime_admission
    FOR EACH ROW EXECUTE FUNCTION guard_recorded_execution_binding();
