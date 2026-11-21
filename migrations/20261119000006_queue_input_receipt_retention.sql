-- Retention metadata only. These IDs never authorize source delivery. The
-- repository derives them from authenticated per-field transfer capabilities;
-- every reader independently verifies the encrypted owning binding.
CREATE FUNCTION queue_input_source_ids_valid(ids BIGINT[]) RETURNS BOOLEAN
LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT cardinality(ids)<=256 AND ids IS NOT DISTINCT FROM
        ARRAY(SELECT DISTINCT value FROM unnest(ids) value WHERE value>0 ORDER BY value)
$$;
ALTER TABLE work_queue_item_configuration_input ADD COLUMN source_receipts BIGINT[] NOT NULL DEFAULT '{}'
    CHECK (queue_input_source_ids_valid(source_receipts));
CREATE INDEX work_queue_configuration_input_receipts ON work_queue_item_configuration_input USING GIN(source_receipts);

CREATE FUNCTION queue_input_holds_receipt(receipt BIGINT) RETURNS BOOLEAN
LANGUAGE SQL STABLE STRICT AS $$
    SELECT EXISTS(SELECT 1 FROM work_queue_item_configuration_input
        WHERE source_receipts @> ARRAY[receipt])
$$;

CREATE FUNCTION guard_queue_input_source_attachment() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE protected BIGINT;
BEGIN
    IF NOT file_evidence_graph_held(false) THEN
        RAISE EXCEPTION 'Queue input sources require graph admission before source protection';
    END IF;
    PERFORM id FROM runtime_input_receipt WHERE id=ANY(NEW.source_receipts) ORDER BY id FOR SHARE;
    GET DIAGNOSTICS protected = ROW_COUNT;
    IF protected<>cardinality(NEW.source_receipts) THEN
        RAISE EXCEPTION 'Queue input source receipt is unavailable';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER queue_input_source_attachment BEFORE INSERT ON work_queue_item_configuration_input
    FOR EACH ROW EXECUTE FUNCTION guard_queue_input_source_attachment();

CREATE FUNCTION guard_queue_input_source_receipt_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT file_evidence_graph_held(true) THEN
        RAISE EXCEPTION 'Queue source reclamation requires exclusive graph admission';
    END IF;
    IF queue_input_holds_receipt(OLD.id) THEN RETURN NULL; END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER queue_input_source_receipt_retention BEFORE DELETE ON runtime_input_receipt
    FOR EACH ROW EXECUTE FUNCTION guard_queue_input_source_receipt_delete();
