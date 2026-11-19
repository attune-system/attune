-- Ordinary files retain their exact allocation generation independently of logs.
-- No reference FK: old versions and retained effects have independent retention.
-- Monotonic boundary evidence survives effect clearance on this assignment.
ALTER TABLE sensor_workload_assignment ADD COLUMN native_effect_seen BOOLEAN NOT NULL DEFAULT FALSE;
UPDATE sensor_workload_assignment SET native_effect_seen=TRUE WHERE native_effect IS NOT NULL;

ALTER TABLE artifact_version
    ADD COLUMN sensor_workload BIGINT,
    ADD COLUMN sensor_generation BIGINT,
    ADD COLUMN sensor_native_intent UUID,
    ADD COLUMN sensor_worker BIGINT,
    ADD COLUMN sensor_worker_instance UUID,
    ADD COLUMN sensor_publication_receipt_bytes BIGINT,
    ADD CONSTRAINT sensor_file_binding_complete CHECK (
        (sensor_workload IS NULL AND sensor_generation IS NULL AND sensor_native_intent IS NULL
          AND sensor_worker IS NULL AND sensor_worker_instance IS NULL AND sensor_publication_receipt_bytes IS NULL)
        OR (sensor_workload IS NOT NULL AND sensor_workload > 0 AND sensor_generation IS NOT NULL AND sensor_generation > 0 AND sensor_native_intent IS NOT NULL
          AND sensor_worker IS NOT NULL AND sensor_worker > 0 AND sensor_worker_instance IS NOT NULL
          AND (sensor_publication_receipt_bytes IS NULL OR sensor_publication_receipt_bytes >= 0)));
CREATE INDEX artifact_version_sensor_native_intent ON artifact_version(sensor_native_intent)
    WHERE sensor_native_intent IS NOT NULL;
-- artifact_version has no history table/column-specific history trigger.

CREATE FUNCTION sensor_native_file_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF OLD.sensor_native_intent IS NOT NULL AND EXISTS (
            SELECT 1 FROM sensor_workload_assignment WHERE workload=OLD.sensor_workload
              AND native_effect->>'intent'=OLD.sensor_native_intent::TEXT) THEN
            RAISE EXCEPTION 'retained native sensor file obligation';
        END IF;
        RETURN OLD;
    END IF;
    IF OLD.sensor_native_intent IS NOT NULL AND NEW.body_state IN ('deleting','cleanup_claimed')
       AND EXISTS(SELECT 1 FROM sensor_workload_assignment WHERE workload=OLD.sensor_workload
         AND native_effect->>'intent'=OLD.sensor_native_intent::TEXT) THEN
        RAISE EXCEPTION 'retained native sensor file body obligation';
    END IF;
    IF OLD.sensor_native_intent IS NOT NULL AND
       ROW(NEW.artifact,NEW.file_path,NEW.execution) IS DISTINCT FROM ROW(OLD.artifact,OLD.file_path,OLD.execution) THEN
        RAISE EXCEPTION 'immutable native sensor file target';
    END IF;
    IF OLD.sensor_native_intent IS NOT NULL AND
       ROW(NEW.sensor_workload,NEW.sensor_generation,NEW.sensor_native_intent,NEW.sensor_worker,NEW.sensor_worker_instance)
       IS DISTINCT FROM ROW(OLD.sensor_workload,OLD.sensor_generation,OLD.sensor_native_intent,OLD.sensor_worker,OLD.sensor_worker_instance) THEN
        RAISE EXCEPTION 'immutable native sensor file binding';
    END IF;
    IF OLD.sensor_publication_receipt_bytes IS NOT NULL AND
       NEW.sensor_publication_receipt_bytes IS DISTINCT FROM OLD.sensor_publication_receipt_bytes THEN
        RAISE EXCEPTION 'immutable native sensor file receipt';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER sensor_native_file_guard BEFORE UPDATE OR DELETE ON artifact_version
    FOR EACH ROW EXECUTE FUNCTION sensor_native_file_guard();

-- Retain recorded original ownership for effects predating this additive field.
-- Missing assignment ownership stays unavailable; never infer a new owner.
UPDATE sensor_workload_assignment SET native_effect=native_effect || jsonb_build_object(
    'worker',worker,'worker_instance',worker_instance)
WHERE native_effect IS NOT NULL AND worker IS NOT NULL AND worker_instance IS NOT NULL;
-- Prior effects have no proof all allocations were bound. Never treat their
-- missing selected rows as a positively empty current-generation inventory.
UPDATE sensor_workload_assignment SET native_effect=native_effect || '{"files_bound":false}'::JSONB
WHERE native_effect IS NOT NULL;

-- Cascaded source retention must not erase the only exact provider intent or
-- make retained ordinary versions appear unowned before receipt-gated cleanup.
CREATE FUNCTION sensor_native_assignment_delete_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.native_effect IS NOT NULL THEN
        RAISE EXCEPTION 'retained native sensor effect obligation';
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER sensor_native_assignment_delete_guard BEFORE DELETE ON sensor_workload_assignment
    FOR EACH ROW EXECUTE FUNCTION sensor_native_assignment_delete_guard();

CREATE FUNCTION sensor_native_boundary_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.native_effect_seen AND NOT NEW.native_effect_seen THEN
        RAISE EXCEPTION 'native sensor boundary cannot revert to legacy';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER sensor_native_boundary_guard BEFORE UPDATE OF native_effect_seen ON sensor_workload_assignment
    FOR EACH ROW EXECUTE FUNCTION sensor_native_boundary_guard();
