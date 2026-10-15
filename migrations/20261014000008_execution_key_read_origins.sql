-- Serialize origin writes without upgrading the admission shared lock held by
-- concurrent credential validation. Runtime IDs remain independent of retention.
CREATE TABLE execution_key_read_origin_fence (
    admission BIGINT PRIMARY KEY REFERENCES runtime_authority_admission(id) ON DELETE CASCADE
);
CREATE TABLE execution_key_read_origin (
    admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE CASCADE,
    origin_digest TEXT NOT NULL CHECK (origin_digest ~ '^[0-9a-f]{64}$'),
    origin JSONB NOT NULL CHECK (jsonb_typeof(origin) = 'object'),
    PRIMARY KEY (admission, origin_digest)
);
CREATE TRIGGER guard_execution_key_read_origin BEFORE UPDATE ON execution_key_read_origin
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
