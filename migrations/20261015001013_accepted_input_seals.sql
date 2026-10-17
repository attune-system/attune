CREATE TABLE runtime_accepted_input_epoch (
    admission BIGINT PRIMARY KEY REFERENCES runtime_authority_admission(id) ON DELETE CASCADE,
    transaction_id TEXT NOT NULL DEFAULT pg_current_xact_id()::TEXT
);
CREATE FUNCTION capture_accepted_input_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.owner_kind = 'execution' THEN
        INSERT INTO runtime_accepted_input_epoch(admission) VALUES (NEW.id);
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER capture_accepted_input_epoch AFTER INSERT ON runtime_authority_admission
    FOR EACH ROW EXECUTE FUNCTION capture_accepted_input_epoch();
CREATE TRIGGER guard_runtime_accepted_input_epoch BEFORE UPDATE ON runtime_accepted_input_epoch
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE TABLE runtime_accepted_input (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    execution BIGINT NOT NULL UNIQUE,
    admission BIGINT NOT NULL UNIQUE REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    body JSONB NOT NULL CHECK(octet_length(body::TEXT) <= 1048576),
    body_digest TEXT NOT NULL,
    encryption_key_hash TEXT NOT NULL
);
CREATE TRIGGER guard_runtime_accepted_input BEFORE UPDATE ON runtime_accepted_input
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TABLE runtime_input_receipt_accepted (
    receipt BIGINT PRIMARY KEY REFERENCES runtime_input_receipt(id) ON DELETE CASCADE,
    accepted BIGINT NOT NULL REFERENCES runtime_accepted_input(id) ON DELETE RESTRICT
);
CREATE INDEX runtime_input_receipt_accepted_parent ON runtime_input_receipt_accepted(accepted);
CREATE TRIGGER guard_runtime_input_receipt_accepted BEFORE UPDATE ON runtime_input_receipt_accepted
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TABLE runtime_accepted_input_delivery (
    accepted BIGINT NOT NULL REFERENCES runtime_accepted_input(id) ON DELETE CASCADE,
    json_path TEXT NOT NULL,
    -- Plain ID: the source may be retained/collected independently. A live
    -- config row's normalized provenance edge keeps its exact receipt alive.
    receipt BIGINT NOT NULL,
    PRIMARY KEY(accepted,json_path)
);
CREATE TRIGGER guard_runtime_accepted_input_delivery BEFORE UPDATE ON runtime_accepted_input_delivery
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

-- Copies keep their ORIGINAL provenance so existing normalized source references
-- pin the same runtime-read/event/configuration parents, without a new origin kind.
CREATE FUNCTION delete_accepted_input_archive() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM execution_secret_value WHERE entity_type = 'accepted_input' AND entity_id = OLD.id;
    RETURN OLD;
END;
$$;
CREATE TRIGGER delete_accepted_input_archive AFTER DELETE ON runtime_accepted_input
    FOR EACH ROW EXECUTE FUNCTION delete_accepted_input_archive();

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
          AND NOT EXISTS(SELECT 1 FROM runtime_accepted_input input WHERE input.admission = admission.id);
    END IF;
    RETURN OLD;
END;
$$;
