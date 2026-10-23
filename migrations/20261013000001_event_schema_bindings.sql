-- Trigger instance parameters and emitted event sections have separate schemas.
ALTER TABLE trigger ADD COLUMN event_config_schema JSONB
    CHECK (event_config_schema IS NULL OR jsonb_typeof(event_config_schema) = 'object');

-- Events use self-contained contracts and independent retention IDs.
CREATE TABLE event_schema_binding (
    event BIGINT PRIMARY KEY CHECK (event > 0),
    contract JSONB NOT NULL CHECK (jsonb_typeof(contract) = 'object'),
    binding_digest TEXT NOT NULL CHECK (binding_digest ~ '^[0-9a-f]{64}$'),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE TRIGGER guard_event_schema_binding BEFORE UPDATE ON event_schema_binding
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE FUNCTION delete_owned_event_schema_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM event_schema_binding WHERE event = OLD.id;
    DELETE FROM execution_secret_value
        WHERE entity_type IN ('event_payload', 'event_config') AND entity_id = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_event_schema_binding AFTER DELETE ON event
    FOR EACH ROW EXECUTE FUNCTION delete_owned_event_schema_binding();
