CREATE TABLE component_configuration_revision (
    id BIGSERIAL PRIMARY KEY,
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('pack', 'sensor')),
    owner_id BIGINT NOT NULL CHECK (owner_id > 0),
    owner_ref TEXT NOT NULL,
    pack_ref TEXT NOT NULL,
    contract JSONB NOT NULL,
    binding_digest TEXT NOT NULL,
    config JSONB NOT NULL CHECK (jsonb_typeof(config) = 'object'),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX component_configuration_owner_idx ON component_configuration_revision (owner_kind, owner_id, id DESC);
CREATE TRIGGER guard_component_configuration_revision BEFORE UPDATE ON component_configuration_revision
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TABLE component_configuration_current (
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('pack', 'sensor')),
    owner_id BIGINT NOT NULL CHECK (owner_id > 0),
    revision BIGINT NOT NULL REFERENCES component_configuration_revision(id),
    PRIMARY KEY (owner_kind, owner_id)
);
CREATE FUNCTION guard_component_configuration_secret_values() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.entity_type = 'component_configuration'
       AND EXISTS (SELECT 1 FROM component_configuration_revision WHERE id = OLD.entity_id) THEN
        RAISE EXCEPTION 'component configuration secret values are immutable';
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_component_configuration_secret_values BEFORE UPDATE OR DELETE ON execution_secret_value
    FOR EACH ROW EXECUTE FUNCTION guard_component_configuration_secret_values();
CREATE FUNCTION delete_component_configuration_owner() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM component_configuration_current WHERE owner_kind = TG_TABLE_NAME AND owner_id = OLD.id;
    WITH removed AS (DELETE FROM component_configuration_revision WHERE owner_kind = TG_TABLE_NAME AND owner_id = OLD.id RETURNING id)
    DELETE FROM execution_secret_value WHERE entity_type = 'component_configuration' AND entity_id IN (SELECT id FROM removed);
    RETURN OLD;
END $$;
CREATE TRIGGER delete_pack_configuration_owner AFTER DELETE ON pack FOR EACH ROW EXECUTE FUNCTION delete_component_configuration_owner();
CREATE TRIGGER delete_sensor_configuration_owner AFTER DELETE ON sensor FOR EACH ROW EXECUTE FUNCTION delete_component_configuration_owner();
