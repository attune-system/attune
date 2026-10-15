CREATE TABLE inquiry_schema_binding (
    entity_id BIGINT PRIMARY KEY CHECK (entity_id > 0),
    binding JSONB NOT NULL CHECK (jsonb_typeof(binding) = 'object'),
    binding_digest TEXT NOT NULL CHECK (binding_digest ~ '^[0-9a-f]{64}$'),
    request_fingerprint TEXT NOT NULL CHECK (request_fingerprint ~ '^[0-9a-f]{64}$'),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TRIGGER guard_inquiry_schema_binding BEFORE UPDATE ON inquiry_schema_binding
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE FUNCTION delete_owned_inquiry_schema_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM inquiry_schema_binding WHERE entity_id = OLD.id;
    DELETE FROM execution_secret_value WHERE entity_type IN ('inquiry_options', 'inquiry_response') AND entity_id = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_inquiry_schema_binding AFTER DELETE ON inquiry
    FOR EACH ROW EXECUTE FUNCTION delete_owned_inquiry_schema_binding();
