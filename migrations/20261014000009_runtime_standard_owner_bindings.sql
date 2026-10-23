-- Standard access is bound to concrete owners in immutable executable metadata,
-- not to mutable action refs supplied in credentials.
CREATE TABLE runtime_standard_owner_binding (
    policy_revision BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE CASCADE,
    invocation_digest TEXT NOT NULL CHECK (invocation_digest ~ '^[0-9a-f]{64}$'),
    owner_type TEXT NOT NULL CHECK (owner_type IN ('pack', 'action')),
    owner_id BIGINT NOT NULL CHECK (owner_id > 0),
    PRIMARY KEY (policy_revision, invocation_digest, owner_type, owner_id)
);
CREATE TRIGGER guard_runtime_standard_owner_binding BEFORE UPDATE ON runtime_standard_owner_binding
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
