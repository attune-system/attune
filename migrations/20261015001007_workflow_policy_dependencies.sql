-- These are authority dependencies, not subscriptions to current publisher state.
CREATE TABLE runtime_workflow_policy_dependency (
    policy_revision BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE CASCADE,
    dependency BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE RESTRICT,
    PRIMARY KEY (policy_revision, dependency),
    CHECK (policy_revision <> dependency)
);
CREATE INDEX runtime_workflow_policy_dependency_target ON runtime_workflow_policy_dependency(dependency);

-- Keep the originating admission's revocation fence across sponsor handoffs and
-- retention of the execution row. Runtime execution IDs retain plain-ID semantics.
CREATE TABLE runtime_admission_dependency (
    admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE CASCADE,
    dependency BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    dependency_kind TEXT NOT NULL DEFAULT 'parent' CHECK (dependency_kind IN ('parent','retry')),
    PRIMARY KEY (admission, dependency),
    CHECK (admission <> dependency)
);
CREATE INDEX runtime_admission_dependency_target ON runtime_admission_dependency(dependency);

CREATE OR REPLACE FUNCTION delete_runtime_authority_admission() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM runtime_authority_admission admission
    WHERE owner_kind = TG_TABLE_NAME AND owner_id = OLD.id
      AND NOT EXISTS (SELECT 1 FROM runtime_admission_dependency edge WHERE edge.dependency = admission.id);
    RETURN OLD;
END;
$$;
