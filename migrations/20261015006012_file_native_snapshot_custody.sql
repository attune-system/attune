-- Requires A 01014/01015. Private archive owners are not public origins.
DO $$ BEGIN
    IF to_regclass('runtime_native_input_snapshot') IS NULL
       OR to_regclass('runtime_native_input_snapshot_parent') IS NULL THEN
        RAISE EXCEPTION 'Native file custody requires A native snapshot and wait observation migrations';
    END IF;
END $$;

CREATE FUNCTION file_evidence_guard_native_archive() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE pinned BOOLEAN;
BEGIN
    IF TG_TABLE_NAME='execution_secret_value' THEN
        IF OLD.entity_type<>'native_input_snapshot' THEN
            IF TG_OP='UPDATE' THEN RETURN NEW; END IF;
            RETURN OLD;
        END IF;
        pinned:=file_evidence_pinned('runtime_native_input_snapshot',OLD.entity_id);
    ELSE
        pinned:=file_evidence_pinned('runtime_authority_admission',OLD.renderer_admission)
          OR EXISTS(SELECT 1 FROM runtime_native_input_snapshot snapshot
            WHERE snapshot.renderer_admission=OLD.renderer_admission
              AND file_evidence_pinned('runtime_native_input_snapshot',snapshot.id));
    END IF;
    IF TG_OP='UPDATE' THEN
        IF pinned THEN RAISE EXCEPTION 'Pinned native snapshot ciphertext/budget is immutable'; END IF;
        RETURN NEW;
    END IF;
    IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Native archive GC requires exclusive graph admission'; END IF;
    IF pinned THEN RETURN NULL; END IF;
    RETURN OLD;
END; $$;

-- A protects live snapshot rows, ciphertext and normalized links. F also keeps
-- orphan ciphertext and the budget identity until exact physical file release.
CREATE TRIGGER file_evidence_native_archive_guard BEFORE DELETE OR UPDATE ON execution_secret_value
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_native_archive();
CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON runtime_native_input_snapshot_budget
FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry();
CREATE TRIGGER file_evidence_native_archive_guard BEFORE DELETE OR UPDATE ON runtime_native_input_snapshot_budget
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_native_archive();

-- Preserve A's normalized native parents AND F's pins when raw owners expire.
-- Bounded collection then reclaims unreferenced children before their ancestors.
CREATE OR REPLACE FUNCTION delete_runtime_authority_admission() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE source_id BIGINT; accepted BOOLEAN := FALSE;
BEGIN
    IF TG_TABLE_NAME = 'execution' THEN
        DELETE FROM runtime_declared_input_receipt WHERE execution = OLD.id;
        DELETE FROM runtime_input_receipt receipt WHERE execution = OLD.id
          AND NOT file_evidence_pinned('runtime_input_receipt',receipt.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_source edge WHERE edge.source_receipt = receipt.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_parent parent WHERE parent.receipt=receipt.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_secret_origin_reference origin WHERE origin.source_id = receipt.id AND origin.source_entity LIKE 'declared_input:%');
    END IF;
    SELECT id INTO source_id FROM runtime_authority_admission WHERE owner_kind = TG_TABLE_NAME AND owner_id = OLD.id;
    IF to_regclass('work_queue_item_ad_hoc_dispatch') IS NOT NULL THEN
        EXECUTE 'SELECT EXISTS(SELECT 1 FROM work_queue_item_ad_hoc_dispatch WHERE source_admission = $1)' INTO accepted USING source_id;
    END IF;
    IF NOT accepted THEN
        DELETE FROM runtime_authority_admission admission WHERE id = source_id
          AND NOT file_evidence_pinned('runtime_authority_admission',admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_admission_dependency edge WHERE edge.dependency = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_declared_input_receipt receipt WHERE receipt.source_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt receipt WHERE receipt.renderer_admission = admission.id OR receipt.destination_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_dependency proof WHERE proof.admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_accepted_input input WHERE input.admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot input WHERE input.renderer_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_admission input WHERE input.admission=admission.id);
    END IF;
    RETURN OLD;
END; $$;
