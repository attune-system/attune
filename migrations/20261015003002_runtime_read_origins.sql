-- A delivery is captured before bytes leave the protected read transaction.
-- References use immutable row IDs, never a mutable execution-wide origin set.
CREATE TABLE execution_runtime_read_fence (
    admission BIGINT PRIMARY KEY REFERENCES runtime_authority_admission(id) ON DELETE CASCADE,
    closed BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE execution_runtime_read_origin (
    id BIGSERIAL PRIMARY KEY,
    admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE CASCADE,
    origin_digest TEXT NOT NULL,
    origin JSONB NOT NULL,
    UNIQUE (admission, origin_digest),
    CHECK (octet_length(origin::TEXT) <= 1048576)
);

CREATE FUNCTION guard_execution_runtime_read_origin() RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'Runtime read origins are immutable';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER execution_runtime_read_origin_immutable BEFORE UPDATE ON execution_runtime_read_origin
FOR EACH ROW EXECUTE FUNCTION guard_execution_runtime_read_origin();

CREATE FUNCTION guard_execution_runtime_read_fence() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.admission IS DISTINCT FROM OLD.admission OR OLD.closed AND NOT NEW.closed THEN
        RAISE EXCEPTION 'Runtime read closure is irreversible';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER execution_runtime_read_fence_guard BEFORE UPDATE ON execution_runtime_read_fence
FOR EACH ROW EXECUTE FUNCTION guard_execution_runtime_read_fence();
