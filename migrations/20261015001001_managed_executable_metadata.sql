-- Metadata revisions reference existing immutable code bytes. They never insert
-- a second executable snapshot under an existing pack release key.
CREATE TABLE managed_executable_metadata_revision (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    pack BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    release BIGINT NOT NULL REFERENCES pack_release(id) ON DELETE CASCADE,
    action BIGINT NOT NULL,
    action_ref TEXT NOT NULL,
    snapshot JSONB NOT NULL,
    snapshot_digest TEXT NOT NULL CHECK (snapshot_digest ~ '^[0-9a-f]{64}$'),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (release, action, snapshot_digest)
);
CREATE INDEX managed_executable_metadata_pack ON managed_executable_metadata_revision(pack);
CREATE TRIGGER guard_managed_executable_metadata BEFORE UPDATE ON managed_executable_metadata_revision
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE TABLE managed_executable_metadata_current (
    action BIGINT PRIMARY KEY REFERENCES action(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL REFERENCES managed_executable_metadata_revision(id) ON DELETE CASCADE
);
CREATE TABLE runtime_authority_metadata_dependency (
    policy_revision BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE CASCADE,
    metadata_revision BIGINT NOT NULL REFERENCES managed_executable_metadata_revision(id) DEFERRABLE INITIALLY DEFERRED,
    PRIMARY KEY (policy_revision, metadata_revision)
);
CREATE INDEX runtime_authority_metadata_dependency_revision ON runtime_authority_metadata_dependency(metadata_revision);
CREATE TRIGGER guard_runtime_authority_metadata_dependency BEFORE UPDATE ON runtime_authority_metadata_dependency
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
