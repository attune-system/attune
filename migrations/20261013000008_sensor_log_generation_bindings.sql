CREATE TABLE sensor_log_generation_binding (
    artifact_version BIGINT PRIMARY KEY CHECK (artifact_version > 0),
    generation_binding BIGINT NOT NULL CHECK (generation_binding > 0),
    generation_digest TEXT NOT NULL CHECK (generation_digest ~ '^[0-9a-f]{64}$'),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX sensor_log_generation_owner_idx ON sensor_log_generation_binding (generation_binding);
CREATE TRIGGER guard_sensor_log_generation_binding BEFORE UPDATE ON sensor_log_generation_binding
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE FUNCTION delete_sensor_log_generation_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM sensor_log_generation_binding WHERE artifact_version = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_sensor_log_generation_binding AFTER DELETE ON artifact_version
    FOR EACH ROW EXECUTE FUNCTION delete_sensor_log_generation_binding();
