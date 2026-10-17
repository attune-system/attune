CREATE TABLE managed_sensor_runtime_state (
    sensor BIGINT PRIMARY KEY REFERENCES sensor(id) ON DELETE CASCADE,
    revoked_at TIMESTAMPTZ
);
CREATE TRIGGER guard_managed_sensor_runtime_state BEFORE UPDATE ON managed_sensor_runtime_state
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_authority_record();

CREATE TABLE managed_sensor_metadata_revision (
    id BIGSERIAL PRIMARY KEY,
    sensor BIGINT NOT NULL REFERENCES sensor(id) ON DELETE CASCADE,
    pack BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    release BIGINT NOT NULL REFERENCES pack_release(id),
    configuration_revision BIGINT REFERENCES component_configuration_revision(id),
    body JSONB NOT NULL CHECK (jsonb_typeof(body) = 'object'),
    body_digest TEXT NOT NULL CHECK (body_digest ~ '^[0-9a-f]{64}$'),
    revoked_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(sensor,release,body_digest)
);
CREATE INDEX managed_sensor_metadata_release_idx ON managed_sensor_metadata_revision(release);
CREATE INDEX managed_sensor_metadata_configuration_idx ON managed_sensor_metadata_revision(configuration_revision);
CREATE TRIGGER guard_managed_sensor_metadata_revision BEFORE UPDATE ON managed_sensor_metadata_revision
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_authority_record();
CREATE TABLE managed_sensor_metadata_current (
    sensor BIGINT PRIMARY KEY REFERENCES sensor(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL REFERENCES managed_sensor_metadata_revision(id)
);
CREATE FUNCTION check_managed_sensor_metadata_owner() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS(SELECT 1 FROM managed_sensor_metadata_revision WHERE id=NEW.revision AND sensor=NEW.sensor) THEN
        RAISE EXCEPTION 'sensor metadata current ownership mismatch';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER check_managed_sensor_metadata_owner BEFORE INSERT OR UPDATE ON managed_sensor_metadata_current
    FOR EACH ROW EXECUTE FUNCTION check_managed_sensor_metadata_owner();
CREATE TABLE managed_sensor_generation (
    workload BIGINT NOT NULL REFERENCES sensor_workload(id) ON DELETE CASCADE,
    generation BIGINT NOT NULL CHECK (generation > 0),
    revision BIGINT NOT NULL REFERENCES managed_sensor_metadata_revision(id),
    PRIMARY KEY(workload,generation)
);
CREATE INDEX managed_sensor_generation_revision_idx ON managed_sensor_generation(revision);
CREATE TRIGGER guard_managed_sensor_generation BEFORE UPDATE ON managed_sensor_generation
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE FUNCTION check_managed_sensor_generation_owner() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS(SELECT 1 FROM sensor_workload w JOIN managed_sensor_metadata_revision r ON r.sensor=w.sensor
        JOIN sensor_workload_assignment a ON a.workload=w.id
        WHERE w.id=NEW.workload AND r.id=NEW.revision AND a.generation=NEW.generation) THEN
        RAISE EXCEPTION 'sensor metadata generation ownership mismatch';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER check_managed_sensor_generation_owner BEFORE INSERT ON managed_sensor_generation
    FOR EACH ROW EXECUTE FUNCTION check_managed_sensor_generation_owner();
CREATE FUNCTION delete_managed_sensor_generation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM managed_sensor_generation WHERE workload = OLD.workload AND generation = OLD.generation;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_managed_sensor_generation AFTER DELETE ON sensor_generation_schema_binding
    FOR EACH ROW EXECUTE FUNCTION delete_managed_sensor_generation();
