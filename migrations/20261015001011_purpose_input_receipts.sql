CREATE TABLE runtime_input_receipt (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    execution BIGINT NOT NULL,
    entity_kind TEXT NOT NULL CHECK (entity_kind IN ('execution_config','execution_result','workflow_variables')),
    destination_purpose TEXT NOT NULL CHECK(destination_purpose IN ('value','condition','control')),
    destination_path TEXT NOT NULL,
    destination_admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    renderer_admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    declaration BIGINT REFERENCES runtime_declared_input_binding(id) ON DELETE RESTRICT,
    proof JSONB NOT NULL CHECK (octet_length(proof::TEXT) <= 1048576),
    proof_digest TEXT NOT NULL,
    encrypted_value JSONB NOT NULL,
    encryption_key_hash TEXT NOT NULL
);
CREATE INDEX runtime_input_receipt_site ON runtime_input_receipt(execution,entity_kind,destination_purpose,destination_path);
CREATE INDEX runtime_input_receipt_renderer ON runtime_input_receipt(renderer_admission);
CREATE INDEX runtime_input_receipt_destination ON runtime_input_receipt(destination_admission);
CREATE TABLE runtime_input_receipt_source (
    receipt BIGINT NOT NULL REFERENCES runtime_input_receipt(id) ON DELETE CASCADE,
    source_receipt BIGINT NOT NULL REFERENCES runtime_input_receipt(id) ON DELETE RESTRICT,
    PRIMARY KEY(receipt,source_receipt), CHECK (source_receipt < receipt)
);
CREATE INDEX runtime_input_receipt_source_target ON runtime_input_receipt_source(source_receipt);
CREATE TABLE runtime_input_configuration_reference (
    receipt BIGINT NOT NULL REFERENCES runtime_input_receipt(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL REFERENCES component_configuration_revision(id) ON DELETE RESTRICT,
    PRIMARY KEY(receipt,revision)
);
CREATE INDEX runtime_input_configuration_revision ON runtime_input_configuration_reference(revision);
CREATE TRIGGER guard_runtime_input_receipt BEFORE UPDATE ON runtime_input_receipt
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TRIGGER guard_runtime_input_receipt_source BEFORE UPDATE ON runtime_input_receipt_source
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

-- These grants authorize only reads for one immutable authored declaration.
-- They are never members of any selected execution permission set.
CREATE TABLE runtime_input_only_approval (
    approval BIGINT PRIMARY KEY REFERENCES runtime_external_approval(id) ON DELETE CASCADE
);
CREATE TABLE runtime_declared_input_approval (
    declaration BIGINT NOT NULL REFERENCES runtime_declared_input_binding(id) ON DELETE CASCADE,
    approval BIGINT NOT NULL REFERENCES runtime_input_only_approval(approval) ON DELETE RESTRICT,
    PRIMARY KEY(declaration,approval)
);
CREATE TABLE runtime_declared_input_key_read (
    declaration BIGINT NOT NULL,
    approval BIGINT NOT NULL,
    origin_digest TEXT NOT NULL,
    origin JSONB NOT NULL,
    target JSONB NOT NULL,
    PRIMARY KEY(declaration,approval,origin_digest),
    FOREIGN KEY(declaration,approval) REFERENCES runtime_declared_input_approval(declaration,approval) ON DELETE CASCADE
);
CREATE TRIGGER guard_runtime_declared_input_approval BEFORE UPDATE ON runtime_declared_input_approval
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TRIGGER guard_runtime_declared_input_key_read BEFORE UPDATE ON runtime_declared_input_key_read
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE OR REPLACE FUNCTION delete_runtime_authority_admission() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE source_id BIGINT; accepted BOOLEAN := FALSE;
BEGIN
    IF TG_TABLE_NAME = 'execution' THEN
        DELETE FROM runtime_declared_input_receipt WHERE execution = OLD.id;
        DELETE FROM runtime_input_receipt receipt WHERE execution = OLD.id
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_source edge WHERE edge.source_receipt = receipt.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_secret_origin_reference origin WHERE origin.source_id = receipt.id AND origin.source_entity LIKE 'declared_input:%');
    END IF;
    SELECT id INTO source_id FROM runtime_authority_admission WHERE owner_kind = TG_TABLE_NAME AND owner_id = OLD.id;
    IF to_regclass('work_queue_item_ad_hoc_dispatch') IS NOT NULL THEN
        EXECUTE 'SELECT EXISTS(SELECT 1 FROM work_queue_item_ad_hoc_dispatch WHERE source_admission = $1)' INTO accepted USING source_id;
    END IF;
    IF NOT accepted THEN
        DELETE FROM runtime_authority_admission admission WHERE id = source_id
          AND NOT EXISTS(SELECT 1 FROM runtime_admission_dependency edge WHERE edge.dependency = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_declared_input_receipt receipt WHERE receipt.source_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt receipt WHERE receipt.renderer_admission = admission.id OR receipt.destination_admission = admission.id);
    END IF;
    RETURN OLD;
END;
$$;
