-- Initial registration is bound only after its source proofs are prepared.
-- No wait/workflow FK is acquired by the source factory's ID reservation.
CREATE TABLE runtime_native_wait_reservation (
    wait BIGINT PRIMARY KEY CHECK (wait > 0),
    renderer BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE CASCADE,
    snapshot BIGINT UNIQUE REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    recipe JSONB NOT NULL,
    recipe_digest TEXT NOT NULL,
    prepared_transaction TEXT NOT NULL,
    CHECK (length(recipe_digest) = 64)
);

CREATE TABLE runtime_native_wait_registration (
    wait BIGINT PRIMARY KEY REFERENCES workflow_task_wait(id) ON DELETE CASCADE,
    renderer BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    selector BIGINT REFERENCES runtime_input_receipt(id) ON DELETE RESTRICT,
    task_path TEXT NOT NULL,
    recipe JSONB NOT NULL,
    recipe_digest TEXT NOT NULL,
    registration_transaction TEXT NOT NULL,
    CHECK (length(recipe_digest) = 64),
    CHECK (task_path <> '')
);

CREATE TRIGGER guard_runtime_native_wait_reservation
    BEFORE UPDATE ON runtime_native_wait_reservation
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TRIGGER guard_runtime_native_wait_registration
    BEFORE UPDATE ON runtime_native_wait_registration
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE FUNCTION guard_native_wait_registration_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM workflow_task_wait WHERE id = OLD.wait) THEN
        RAISE EXCEPTION 'Original native wait registration is immutable while its wait exists';
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER guard_native_wait_registration_delete
    BEFORE DELETE ON runtime_native_wait_registration
    FOR EACH ROW EXECUTE FUNCTION guard_native_wait_registration_delete();

COMMENT ON TABLE runtime_native_wait_registration IS
    'Repository-issued original initial wait registration, bound atomically with actual wait insertion and any native release';
