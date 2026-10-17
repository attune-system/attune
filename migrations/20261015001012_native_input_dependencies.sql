CREATE TABLE runtime_input_receipt_dependency (
    receipt BIGINT NOT NULL REFERENCES runtime_input_receipt(id) ON DELETE CASCADE,
    admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    PRIMARY KEY(receipt,admission)
);
CREATE INDEX runtime_input_receipt_dependency_admission ON runtime_input_receipt_dependency(admission);
CREATE TRIGGER guard_runtime_input_receipt_dependency BEFORE UPDATE ON runtime_input_receipt_dependency
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
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_dependency proof WHERE proof.admission = admission.id);
    END IF;
    RETURN OLD;
END;
$$;
