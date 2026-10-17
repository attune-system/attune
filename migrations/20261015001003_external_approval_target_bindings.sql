-- Fixed approval scope includes concrete identities at issue time. Human issuer
-- status and later ref reuse cannot replace those identities.
CREATE TABLE runtime_external_approval_target_binding (
    approval BIGINT NOT NULL REFERENCES runtime_external_approval(id) ON DELETE CASCADE,
    grant_digest TEXT NOT NULL CHECK (grant_digest ~ '^[0-9a-f]{64}$'),
    binding JSONB NOT NULL,
    PRIMARY KEY (approval,grant_digest)
);
CREATE TRIGGER guard_runtime_external_approval_target_binding BEFORE UPDATE ON runtime_external_approval_target_binding
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
