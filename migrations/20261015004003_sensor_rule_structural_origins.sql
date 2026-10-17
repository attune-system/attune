-- No historical backfill: names in old rows are not proof of structural ownership.
CREATE TABLE sensor_rule_structural_origin (
    source_entity TEXT NOT NULL CHECK(source_entity IN ('sensor_generation_rule_config','sensor_rule_delivery_config')),
    source_id BIGINT NOT NULL CHECK(source_id > 0),
    rule_id BIGINT NOT NULL CHECK(rule_id > 0),
    generation_binding BIGINT NOT NULL REFERENCES sensor_generation_schema_binding(id) ON DELETE CASCADE,
    dispatch_policy_revision BIGINT REFERENCES pack_runtime_policy_revision(id),
    dispatch_source_release BIGINT REFERENCES pack_release(id),
    proof JSONB NOT NULL CHECK(jsonb_typeof(proof)='object'),
    proof_digest TEXT NOT NULL CHECK(proof_digest ~ '^[0-9a-f]{64}$'),
    PRIMARY KEY(source_entity,source_id,rule_id)
);
CREATE INDEX sensor_rule_structural_policy_idx ON sensor_rule_structural_origin(dispatch_policy_revision);
CREATE INDEX sensor_rule_structural_release_idx ON sensor_rule_structural_origin(dispatch_source_release);
CREATE INDEX sensor_rule_structural_generation_idx ON sensor_rule_structural_origin(generation_binding);
CREATE TRIGGER guard_sensor_rule_structural_origin BEFORE UPDATE ON sensor_rule_structural_origin
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE FUNCTION delete_sensor_rule_structural_origin() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM sensor_rule_structural_origin
        WHERE source_entity = CASE TG_TABLE_NAME WHEN 'sensor_generation_schema_binding'
            THEN 'sensor_generation_rule_config' ELSE 'sensor_rule_delivery_config' END
        AND source_id=OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_generation_rule_structural_origin AFTER DELETE ON sensor_generation_schema_binding
    FOR EACH ROW EXECUTE FUNCTION delete_sensor_rule_structural_origin();
CREATE TRIGGER delete_delivery_rule_structural_origin AFTER DELETE ON sensor_rule_config_delivery
    FOR EACH ROW EXECUTE FUNCTION delete_sensor_rule_structural_origin();
