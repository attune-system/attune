CREATE TABLE sensor_log_rule_delivery_source (
    artifact_version BIGINT NOT NULL CHECK (artifact_version > 0),
    delivery BIGINT NOT NULL CHECK (delivery > 0),
    PRIMARY KEY (artifact_version, delivery)
);
CREATE INDEX sensor_log_rule_delivery_owner_idx ON sensor_log_rule_delivery_source (delivery);
CREATE TRIGGER guard_sensor_log_rule_delivery_source BEFORE UPDATE ON sensor_log_rule_delivery_source
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE OR REPLACE FUNCTION delete_sensor_log_generation_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM sensor_log_rule_delivery_source WHERE artifact_version = OLD.id;
    DELETE FROM sensor_log_generation_binding WHERE artifact_version = OLD.id;
    RETURN OLD;
END $$;
