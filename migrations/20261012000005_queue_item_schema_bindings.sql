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
    DELETE FROM execution_secret_value WHERE entity_type = 'queue_item_payload' AND entity_id = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_queue_item_secret_values AFTER DELETE ON work_queue_item
    FOR EACH ROW EXECUTE FUNCTION delete_queue_item_secret_values();
