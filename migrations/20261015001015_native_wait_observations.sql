CREATE TABLE runtime_native_wait_release (
    -- Plain wait identity, retained independently of its mutable native owner.
    wait BIGINT PRIMARY KEY,
    snapshot BIGINT NOT NULL UNIQUE REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    release_transaction TEXT NOT NULL
);
CREATE TRIGGER guard_runtime_native_wait_release BEFORE UPDATE ON runtime_native_wait_release
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TABLE runtime_native_input_snapshot_parent (
    snapshot BIGINT NOT NULL REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    receipt BIGINT NOT NULL REFERENCES runtime_input_receipt(id) ON DELETE RESTRICT,
    PRIMARY KEY(snapshot,receipt)
);
CREATE INDEX runtime_native_input_snapshot_parent_receipt ON runtime_native_input_snapshot_parent(receipt);
CREATE TRIGGER guard_runtime_native_input_snapshot_parent BEFORE UPDATE ON runtime_native_input_snapshot_parent
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TABLE runtime_native_input_snapshot_admission (
    snapshot BIGINT NOT NULL REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    PRIMARY KEY(snapshot,admission)
);
CREATE INDEX runtime_native_input_snapshot_admission_source ON runtime_native_input_snapshot_admission(admission);
CREATE TRIGGER guard_runtime_native_input_snapshot_admission BEFORE UPDATE ON runtime_native_input_snapshot_admission
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE FUNCTION delete_native_input_snapshot_archive() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM execution_secret_value WHERE entity_type='native_input_snapshot' AND entity_id=OLD.id;
    RETURN OLD;
END;
$$;
CREATE TRIGGER delete_native_input_snapshot_archive AFTER DELETE ON runtime_native_input_snapshot
    FOR EACH ROW EXECUTE FUNCTION delete_native_input_snapshot_archive();

-- F's graph entry functions are installed later in the combined migration set.
-- Resolve them at runtime rather than bypassing the exclusive reclamation gate.
CREATE FUNCTION native_input_snapshot_delete_entry() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM file_evidence_gc_entry();
    RETURN NULL;
END;
$$;
CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON runtime_native_input_snapshot
    FOR EACH STATEMENT EXECUTE FUNCTION native_input_snapshot_delete_entry();
CREATE FUNCTION guard_native_input_snapshot_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF file_evidence_pinned('runtime_native_input_snapshot',OLD.id) THEN RETURN NULL; END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER file_evidence_guard BEFORE DELETE ON runtime_native_input_snapshot
    FOR EACH ROW EXECUTE FUNCTION guard_native_input_snapshot_delete();
CREATE FUNCTION guard_native_snapshot_secondary_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='runtime_input_receipt_snapshot' THEN
        IF EXISTS(SELECT 1 FROM runtime_input_receipt WHERE id=OLD.receipt) THEN
            RAISE EXCEPTION 'Native input receipt snapshot dependency is immutable';
        END IF;
    ELSIF EXISTS(SELECT 1 FROM runtime_native_input_snapshot WHERE id=OLD.snapshot) THEN
        RAISE EXCEPTION 'Native input snapshot evidence is immutable';
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER guard_native_snapshot_secondary_delete BEFORE DELETE ON runtime_input_receipt_snapshot
    FOR EACH ROW EXECUTE FUNCTION guard_native_snapshot_secondary_delete();
CREATE TRIGGER guard_native_snapshot_secondary_delete BEFORE DELETE ON runtime_native_wait_release
    FOR EACH ROW EXECUTE FUNCTION guard_native_snapshot_secondary_delete();
CREATE TRIGGER guard_native_snapshot_secondary_delete BEFORE DELETE ON runtime_native_input_snapshot_parent
    FOR EACH ROW EXECUTE FUNCTION guard_native_snapshot_secondary_delete();
CREATE TRIGGER guard_native_snapshot_secondary_delete BEFORE DELETE ON runtime_native_input_snapshot_admission
    FOR EACH ROW EXECUTE FUNCTION guard_native_snapshot_secondary_delete();
CREATE FUNCTION guard_native_snapshot_archive() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.entity_type='native_input_snapshot' AND EXISTS(SELECT 1 FROM runtime_native_input_snapshot WHERE id=OLD.entity_id) THEN
        IF TG_OP='DELETE' OR to_jsonb(NEW) IS DISTINCT FROM to_jsonb(OLD) THEN
            RAISE EXCEPTION 'Native input snapshot ciphertext is immutable';
        END IF;
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_native_snapshot_archive BEFORE DELETE OR UPDATE ON execution_secret_value
    FOR EACH ROW EXECUTE FUNCTION guard_native_snapshot_archive();

CREATE OR REPLACE FUNCTION delete_runtime_authority_admission() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE source_id BIGINT; accepted BOOLEAN := FALSE;
BEGIN
    IF TG_TABLE_NAME = 'execution' THEN
        DELETE FROM runtime_declared_input_receipt WHERE execution = OLD.id;
        DELETE FROM runtime_input_receipt receipt WHERE execution = OLD.id
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
          AND NOT EXISTS(SELECT 1 FROM runtime_admission_dependency edge WHERE edge.dependency = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_declared_input_receipt receipt WHERE receipt.source_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt receipt WHERE receipt.renderer_admission = admission.id OR receipt.destination_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_dependency proof WHERE proof.admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_accepted_input input WHERE input.admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot input WHERE input.renderer_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_admission input WHERE input.admission=admission.id);
    END IF;
    RETURN OLD;
END;
$$;
