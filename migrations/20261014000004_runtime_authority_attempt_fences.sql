CREATE TABLE runtime_authority_execution_attempt (
    execution BIGINT NOT NULL CHECK (execution > 0),
    admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE CASCADE,
    attempt UUID NOT NULL CHECK (attempt <> '00000000-0000-0000-0000-000000000000'::UUID),
    PRIMARY KEY (execution, attempt)
);
CREATE INDEX runtime_authority_execution_attempt_admission_idx
    ON runtime_authority_execution_attempt(admission);
CREATE TRIGGER guard_runtime_authority_execution_attempt BEFORE UPDATE ON runtime_authority_execution_attempt
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE TABLE runtime_authority_execution_fence (
    execution BIGINT PRIMARY KEY CHECK (execution > 0),
    admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE CASCADE,
    attempt UUID NOT NULL CHECK (attempt <> '00000000-0000-0000-0000-000000000000'::UUID)
);
CREATE INDEX runtime_authority_execution_fence_admission_idx
    ON runtime_authority_execution_fence(admission);

INSERT INTO runtime_authority_execution_fence (execution, admission, attempt)
    SELECT owner_id, id, (authority->>'attempt')::UUID
    FROM runtime_authority_admission
    WHERE owner_kind = 'execution';
INSERT INTO runtime_authority_execution_attempt (execution, admission, attempt)
    SELECT execution, admission, attempt FROM runtime_authority_execution_fence;

CREATE FUNCTION guard_runtime_authority_execution_fence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.execution IS DISTINCT FROM OLD.execution OR NEW.admission IS DISTINCT FROM OLD.admission THEN
        RAISE EXCEPTION 'execution authority cannot be rebound';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_runtime_authority_execution_fence BEFORE UPDATE ON runtime_authority_execution_fence
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_authority_execution_fence();

COMMENT ON COLUMN runtime_authority_execution_fence.execution IS
    'Plain execution ID, cleaned up through its immutable admission rather than an execution FK';
COMMENT ON COLUMN runtime_authority_execution_fence.attempt IS
    'Current worker attempt; rotation fences previous credentials without rewriting admitted grants';
