ALTER TABLE sensor_generation_schema_binding ADD COLUMN initial_rules JSONB NOT NULL DEFAULT '{}'::jsonb
    CHECK (jsonb_typeof(initial_rules) = 'object');

CREATE OR REPLACE FUNCTION guard_sensor_generation_secret_values() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.entity_type IN ('sensor_generation_config', 'sensor_generation_rule_config')
       AND EXISTS (SELECT 1 FROM sensor_generation_schema_binding WHERE id = OLD.entity_id) THEN
        RAISE EXCEPTION 'sensor generation secret values are immutable';
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION delete_sensor_generation_schema_bindings() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    WITH removed AS (DELETE FROM sensor_generation_schema_binding WHERE workload = OLD.id RETURNING id)
    DELETE FROM execution_secret_value WHERE entity_type IN ('sensor_generation_config', 'sensor_generation_rule_config')
        AND entity_id IN (SELECT id FROM removed);
    RETURN OLD;
END $$;
