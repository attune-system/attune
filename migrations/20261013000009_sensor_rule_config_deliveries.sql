CREATE TABLE sensor_rule_config_delivery (
    id BIGSERIAL PRIMARY KEY,
    generation_binding BIGINT NOT NULL CHECK (generation_binding > 0),
    rule BIGINT NOT NULL CHECK (rule > 0),
    rule_ref TEXT NOT NULL,
    pack_ref TEXT NOT NULL,
    trigger_ref TEXT NOT NULL,
    fingerprint TEXT NOT NULL CHECK (fingerprint ~ '^[0-9a-f]{64}$'),
    config JSONB NOT NULL CHECK (jsonb_typeof(config) = 'object'),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (generation_binding, rule, fingerprint)
);
CREATE INDEX sensor_rule_delivery_generation_idx ON sensor_rule_config_delivery (generation_binding, id);
CREATE TRIGGER guard_sensor_rule_config_delivery BEFORE UPDATE ON sensor_rule_config_delivery
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE FUNCTION guard_sensor_rule_delivery_secret_values() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.entity_type = 'sensor_rule_delivery_config'
       AND EXISTS (SELECT 1 FROM sensor_rule_config_delivery WHERE id = OLD.entity_id) THEN
        RAISE EXCEPTION 'sensor rule delivery secret values are immutable';
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_sensor_rule_delivery_secret_values BEFORE UPDATE OR DELETE ON execution_secret_value
    FOR EACH ROW EXECUTE FUNCTION guard_sensor_rule_delivery_secret_values();

CREATE FUNCTION delete_sensor_rule_config_deliveries() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    WITH removed AS (DELETE FROM sensor_rule_config_delivery WHERE generation_binding = OLD.id RETURNING id)
    DELETE FROM execution_secret_value WHERE entity_type = 'sensor_rule_delivery_config'
        AND entity_id IN (SELECT id FROM removed);
    RETURN OLD;
END $$;
CREATE TRIGGER delete_sensor_rule_config_deliveries AFTER DELETE ON sensor_generation_schema_binding
    FOR EACH ROW EXECUTE FUNCTION delete_sensor_rule_config_deliveries();
