-- Policies retain their sponsoring release and every executable release they admit.
-- Runtime-owner references remain plain IDs in runtime_authority_admission.
CREATE TABLE runtime_authority_release_dependency (
    policy_revision BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE CASCADE,
    release BIGINT NOT NULL REFERENCES pack_release(id) DEFERRABLE INITIALLY DEFERRED,
    PRIMARY KEY (policy_revision, release)
);
CREATE INDEX runtime_authority_release_dependency_release_idx
    ON runtime_authority_release_dependency(release, policy_revision);

INSERT INTO runtime_authority_release_dependency (policy_revision, release)
    SELECT id, release FROM pack_runtime_policy_revision
    UNION
    SELECT p.id, (invocation->>'target_release_id')::BIGINT
    FROM pack_runtime_policy_revision p
    CROSS JOIN LATERAL jsonb_array_elements(p.policy->'invocations') invocation;

CREATE TRIGGER guard_runtime_authority_release_dependency
    BEFORE UPDATE ON runtime_authority_release_dependency
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
