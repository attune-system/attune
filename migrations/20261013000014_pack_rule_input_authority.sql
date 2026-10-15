CREATE TABLE pack_runtime_identity (
    pack BIGINT PRIMARY KEY REFERENCES pack(id) ON DELETE CASCADE,
    identity BIGINT NOT NULL UNIQUE REFERENCES identity(id)
);
CREATE FUNCTION freeze_deleted_pack_runtime_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE identity SET frozen = true WHERE id = OLD.identity;
    RETURN OLD;
END $$;
CREATE TRIGGER freeze_deleted_pack_runtime_identity AFTER DELETE ON pack_runtime_identity FOR EACH ROW EXECUTE FUNCTION freeze_deleted_pack_runtime_identity();
CREATE TABLE rule_input_permission_refs (
    rule BIGINT PRIMARY KEY REFERENCES rule(id) ON DELETE CASCADE,
    refs TEXT[] NOT NULL
);
CREATE TABLE rule_runtime_authority_revision (
    id BIGSERIAL PRIMARY KEY,
    rule BIGINT NOT NULL CHECK (rule > 0),
    authority JSONB NOT NULL CHECK (jsonb_typeof(authority) = 'object'),
    authority_digest TEXT NOT NULL CHECK (authority_digest ~ '^[0-9a-f]{64}$'),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX rule_runtime_authority_owner_idx ON rule_runtime_authority_revision (rule, id);
CREATE TRIGGER guard_rule_runtime_authority_revision BEFORE UPDATE ON rule_runtime_authority_revision
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TABLE rule_runtime_authority_current (
    rule BIGINT PRIMARY KEY REFERENCES rule(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL REFERENCES rule_runtime_authority_revision(id)
);
CREATE TABLE runtime_input_authority (
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('enforcement', 'execution')),
    owner_id BIGINT NOT NULL CHECK (owner_id > 0),
    authority JSONB NOT NULL CHECK (jsonb_typeof(authority) = 'object'),
    authority_digest TEXT NOT NULL CHECK (authority_digest ~ '^[0-9a-f]{64}$'),
    PRIMARY KEY (owner_kind, owner_id)
);
CREATE TRIGGER guard_runtime_input_authority BEFORE UPDATE ON runtime_input_authority
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE FUNCTION delete_runtime_input_authority() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM runtime_input_authority WHERE owner_kind = TG_TABLE_NAME AND owner_id = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_enforcement_input_authority AFTER DELETE ON enforcement FOR EACH ROW EXECUTE FUNCTION delete_runtime_input_authority();
CREATE TRIGGER delete_execution_input_authority AFTER DELETE ON execution FOR EACH ROW EXECUTE FUNCTION delete_runtime_input_authority();

CREATE FUNCTION inherit_runtime_input_authority() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO runtime_input_authority (owner_kind,owner_id,authority,authority_digest)
        SELECT 'execution', NEW.id, source.authority, source.authority_digest FROM runtime_input_authority source
        WHERE (source.authority->>'identity_id')::bigint = NEW.executor
          AND ((source.owner_kind = 'execution' AND source.owner_id = NEW.parent)
            OR (source.owner_kind = 'enforcement' AND source.owner_id = NEW.enforcement)
            OR (source.owner_kind = 'execution' AND source.owner_id = NEW.original_execution))
        ORDER BY CASE WHEN source.owner_kind = 'execution' AND source.owner_id = NEW.parent THEN 0
                      WHEN source.owner_kind = 'enforcement' THEN 1 ELSE 2 END
        LIMIT 1;
    RETURN NEW;
END $$;
CREATE TRIGGER inherit_runtime_input_authority AFTER INSERT ON execution FOR EACH ROW EXECUTE FUNCTION inherit_runtime_input_authority();

CREATE FUNCTION delete_rule_authority_revisions() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM rule_runtime_authority_current WHERE rule = OLD.id;
    DELETE FROM rule_runtime_authority_revision WHERE rule = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_rule_authority_revisions AFTER DELETE ON rule FOR EACH ROW EXECUTE FUNCTION delete_rule_authority_revisions();
