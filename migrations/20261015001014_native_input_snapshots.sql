CREATE TABLE runtime_native_input_snapshot (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    renderer_admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    policy_revision BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE RESTRICT,
    body JSONB NOT NULL CHECK (octet_length(body::TEXT) <= 1048576),
    body_digest TEXT NOT NULL,
    encrypted_value JSONB NOT NULL,
    encryption_key_hash TEXT NOT NULL
);
CREATE TRIGGER guard_runtime_native_input_snapshot BEFORE UPDATE ON runtime_native_input_snapshot
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TABLE runtime_native_input_snapshot_budget (
    renderer_admission BIGINT PRIMARY KEY REFERENCES runtime_authority_admission(id) ON DELETE CASCADE
);
CREATE TABLE runtime_input_receipt_snapshot (
    receipt BIGINT NOT NULL REFERENCES runtime_input_receipt(id) ON DELETE CASCADE,
    snapshot BIGINT NOT NULL REFERENCES runtime_native_input_snapshot(id) ON DELETE RESTRICT,
    PRIMARY KEY(receipt,snapshot)
);
CREATE INDEX runtime_input_receipt_snapshot_parent ON runtime_input_receipt_snapshot(snapshot);
CREATE TRIGGER guard_runtime_input_receipt_snapshot BEFORE UPDATE ON runtime_input_receipt_snapshot
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
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt receipt WHERE receipt.renderer_admission = admission.id OR receipt.destination_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_dependency proof WHERE proof.admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_accepted_input input WHERE input.admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot input WHERE input.renderer_admission = admission.id);
    END IF;
    RETURN OLD;
END;
$$;
