CREATE INDEX sensor_generation_release_idx ON sensor_generation_schema_binding
    (((binding->'executable'->'release'->>'id')::BIGINT));
CREATE INDEX sensor_generation_retention_idx ON sensor_generation_schema_binding (created, id);

CREATE FUNCTION guard_sensor_generation_secret_values() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.entity_type = 'sensor_generation_config'
       AND EXISTS (SELECT 1 FROM sensor_generation_schema_binding WHERE id = OLD.entity_id) THEN
        RAISE EXCEPTION 'sensor generation secret values are immutable';
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_sensor_generation_secret_values BEFORE UPDATE OR DELETE ON execution_secret_value
    FOR EACH ROW EXECUTE FUNCTION guard_sensor_generation_secret_values();

CREATE OR REPLACE FUNCTION delete_sensor_generation_schema_bindings() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    WITH removed AS (DELETE FROM sensor_generation_schema_binding WHERE workload = OLD.id RETURNING id)
    DELETE FROM execution_secret_value WHERE entity_type = 'sensor_generation_config'
        AND entity_id IN (SELECT id FROM removed);
    RETURN OLD;
END $$;
