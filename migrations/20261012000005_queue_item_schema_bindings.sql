ALTER TABLE work_queue_item ADD COLUMN schema_binding JSONB
    CHECK (schema_binding IS NULL OR jsonb_typeof(schema_binding) = 'object');

CREATE FUNCTION guard_queue_item_schema_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.schema_binding IS DISTINCT FROM OLD.schema_binding THEN
        RAISE EXCEPTION 'queue item admission schema bindings are immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_queue_item_schema_binding BEFORE UPDATE ON work_queue_item
    FOR EACH ROW EXECUTE FUNCTION guard_queue_item_schema_binding();

CREATE FUNCTION delete_queue_item_secret_values() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM execution_secret_value WHERE entity_type IN ('queue_item_payload', 'queue_item_ack_summary', 'queue_item_last_error', 'queue_item_result') AND entity_id = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_queue_item_secret_values AFTER DELETE ON work_queue_item
    FOR EACH ROW EXECUTE FUNCTION delete_queue_item_secret_values();

-- Public queue metadata mutations invalidate their old protected projections.
-- Completion writes its freshly prepared encrypted copies after the row update
-- in the same transaction, so no stale value can restore into newer metadata.
CREATE FUNCTION clear_changed_queue_item_output_secrets() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.ack_summary IS DISTINCT FROM OLD.ack_summary THEN
        DELETE FROM execution_secret_value WHERE entity_type = 'queue_item_ack_summary' AND entity_id = OLD.id;
    END IF;
    IF NEW.last_error IS DISTINCT FROM OLD.last_error THEN
        DELETE FROM execution_secret_value WHERE entity_type = 'queue_item_last_error' AND entity_id = OLD.id;
    END IF;
    IF NEW.ack_summary IS DISTINCT FROM OLD.ack_summary
       OR NEW.last_error IS DISTINCT FROM OLD.last_error
       OR NEW.status IS DISTINCT FROM OLD.status THEN
        DELETE FROM execution_secret_value WHERE entity_type = 'queue_item_result' AND entity_id = OLD.id;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER clear_changed_queue_item_output_secrets AFTER UPDATE ON work_queue_item
    FOR EACH ROW EXECUTE FUNCTION clear_changed_queue_item_output_secrets();
