ALTER TABLE runtime_admission_dependency DROP CONSTRAINT runtime_admission_dependency_dependency_kind_check;
ALTER TABLE runtime_admission_dependency ADD CHECK (dependency_kind IN ('parent','retry','accepted_item'));
CREATE TABLE runtime_accepted_batch_manifest (
    admission BIGINT PRIMARY KEY REFERENCES runtime_authority_admission(id) ON DELETE CASCADE,
    item_ids BIGINT[] NOT NULL CHECK (cardinality(item_ids) BETWEEN 1 AND 256)
);
CREATE TABLE runtime_accepted_batch_item (
    admission BIGINT NOT NULL REFERENCES runtime_accepted_batch_manifest(admission) ON DELETE CASCADE,
    item BIGINT NOT NULL,
    source_admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    target_policy BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE RESTRICT,
    proof JSONB NOT NULL,
    proof_digest TEXT NOT NULL,
    PRIMARY KEY(admission,item)
);
CREATE INDEX runtime_accepted_batch_target_policy ON runtime_accepted_batch_item(target_policy);
CREATE INDEX runtime_accepted_batch_source ON runtime_accepted_batch_item(source_admission);
CREATE TRIGGER guard_runtime_accepted_batch_manifest BEFORE UPDATE ON runtime_accepted_batch_manifest
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TRIGGER guard_runtime_accepted_batch_item BEFORE UPDATE ON runtime_accepted_batch_item
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

-- B's item proof may precede dispatch. Retain its source admission when the
-- source execution expires, even before an accepted batch has been constructed.
CREATE OR REPLACE FUNCTION delete_runtime_authority_admission() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE source_id BIGINT; accepted BOOLEAN := FALSE;
BEGIN
    IF TG_TABLE_NAME = 'execution' THEN
        DELETE FROM runtime_declared_input_receipt WHERE execution = OLD.id;
    END IF;
    SELECT id INTO source_id FROM runtime_authority_admission WHERE owner_kind = TG_TABLE_NAME AND owner_id = OLD.id;
    IF to_regclass('work_queue_item_ad_hoc_dispatch') IS NOT NULL THEN
        EXECUTE 'SELECT EXISTS(SELECT 1 FROM work_queue_item_ad_hoc_dispatch WHERE source_admission = $1)' INTO accepted USING source_id;
    END IF;
    IF NOT accepted THEN
        DELETE FROM runtime_authority_admission admission WHERE id = source_id
          AND NOT EXISTS(SELECT 1 FROM runtime_admission_dependency edge WHERE edge.dependency = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_declared_input_receipt receipt WHERE receipt.source_admission = admission.id);
    END IF;
    RETURN OLD;
END;
$$;
