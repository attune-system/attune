-- Key schemas are value assignments, not executable contracts. Existing
-- bindings are replaced only by explicit schema changes or rebinding.
CREATE TABLE key_schema_assignment (
    key_id BIGINT PRIMARY KEY REFERENCES key(id) ON DELETE CASCADE,
    authored_schema JSONB NOT NULL CHECK (jsonb_typeof(authored_schema) IN ('object', 'boolean')),
    contract JSONB NOT NULL CHECK (jsonb_typeof(contract) = 'object'),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
