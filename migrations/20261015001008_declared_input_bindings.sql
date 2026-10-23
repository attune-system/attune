CREATE TABLE runtime_declared_input_binding (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    policy_revision BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE CASCADE,
    invocation_digest TEXT NOT NULL,
    declaration JSONB NOT NULL,
    declaration_digest TEXT NOT NULL,
    UNIQUE(policy_revision, invocation_digest, declaration_digest)
);
-- Runtime IDs deliberately have no execution FK. A receipt retains the source
-- policy/declaration and its revocation dependency independently of row retention.
CREATE TABLE runtime_declared_input_receipt (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    execution BIGINT NOT NULL,
    destination_path TEXT NOT NULL,
    source_admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    declaration BIGINT NOT NULL REFERENCES runtime_declared_input_binding(id) ON DELETE RESTRICT,
    origin JSONB NOT NULL,
    origin_digest TEXT NOT NULL,
    UNIQUE(execution, destination_path, source_admission, declaration, origin_digest)
);
CREATE INDEX runtime_declared_input_receipt_execution ON runtime_declared_input_receipt(execution);

CREATE OR REPLACE FUNCTION delete_runtime_authority_admission() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME = 'execution' THEN
        DELETE FROM runtime_declared_input_receipt WHERE execution = OLD.id;
    END IF;
    DELETE FROM runtime_authority_admission admission
    WHERE owner_kind = TG_TABLE_NAME AND owner_id = OLD.id
      AND NOT EXISTS (SELECT 1 FROM runtime_admission_dependency edge WHERE edge.dependency = admission.id)
      AND NOT EXISTS (SELECT 1 FROM runtime_declared_input_receipt receipt WHERE receipt.source_admission = admission.id);
    RETURN OLD;
END;
$$;
