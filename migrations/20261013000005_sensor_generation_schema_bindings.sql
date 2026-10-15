-- Workload generations, not installer identities, own running sensor contracts.
CREATE TABLE sensor_generation_schema_binding (
    id BIGSERIAL PRIMARY KEY,
    workload BIGINT NOT NULL CHECK (workload > 0),
    generation BIGINT NOT NULL CHECK (generation > 0),
    binding JSONB NOT NULL CHECK (jsonb_typeof(binding) = 'object'),
    binding_digest TEXT NOT NULL CHECK (binding_digest ~ '^[0-9a-f]{64}$'),
    config JSONB NOT NULL CHECK (jsonb_typeof(config) = 'object'),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (workload, generation)
);
CREATE TRIGGER guard_sensor_generation_schema_binding BEFORE UPDATE ON sensor_generation_schema_binding
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE FUNCTION delete_sensor_generation_schema_bindings() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM execution_secret_value WHERE entity_type = 'sensor_generation_config'
        AND entity_id IN (SELECT id FROM sensor_generation_schema_binding WHERE workload = OLD.id);
    DELETE FROM sensor_generation_schema_binding WHERE workload = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_sensor_generation_schema_bindings AFTER DELETE ON sensor_workload
    FOR EACH ROW EXECUTE FUNCTION delete_sensor_generation_schema_bindings();
