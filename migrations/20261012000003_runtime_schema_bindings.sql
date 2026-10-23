-- Self-contained runtime contracts have independent owner IDs, like secret
-- values. An execution never depends on its enforcement surviving retention.
CREATE TABLE runtime_schema_binding (
    entity_kind TEXT NOT NULL CHECK (entity_kind IN ('enforcement', 'execution')),
    entity_id BIGINT NOT NULL CHECK (entity_id > 0),
    contract JSONB NOT NULL CHECK (jsonb_typeof(contract) = 'object'),
    binding_digest TEXT NOT NULL CHECK (binding_digest ~ '^[0-9a-f]{64}$'),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (entity_kind, entity_id)
);

CREATE FUNCTION guard_runtime_schema_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'runtime schema bindings are immutable';
END $$;
CREATE TRIGGER guard_runtime_schema_binding BEFORE UPDATE ON runtime_schema_binding
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

CREATE FUNCTION delete_owned_runtime_schema_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM runtime_schema_binding WHERE entity_kind = TG_ARGV[0] AND entity_id = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_enforcement_schema_binding AFTER DELETE ON enforcement
    FOR EACH ROW EXECUTE FUNCTION delete_owned_runtime_schema_binding('enforcement');
CREATE TRIGGER delete_execution_schema_binding AFTER DELETE ON execution
    FOR EACH ROW EXECUTE FUNCTION delete_owned_runtime_schema_binding('execution');
