-- Recorded consumers require structural owner IDs captured at the write, not
-- a later lookup of artifact.owner after a component ref has been reused.
SET search_path TO attune, public;

CREATE TABLE artifact_runtime_owner_binding (
    artifact BIGINT PRIMARY KEY REFERENCES artifact(id) ON DELETE CASCADE,
    owner_type owner_type_enum NOT NULL,
    owner_ref TEXT NOT NULL,
    owner_id BIGINT,
    managing_pack BIGINT,
    CHECK (owner_id IS NULL OR owner_id > 0),
    CHECK (managing_pack IS NULL OR managing_pack > 0),
    CHECK ((owner_type = 'system') = (owner_id IS NULL)),
    CHECK (owner_type NOT IN ('system', 'identity') OR managing_pack IS NULL),
    CHECK (owner_type <> 'pack' OR managing_pack = owner_id)
);

CREATE FUNCTION guard_artifact_runtime_owner_binding() RETURNS TRIGGER AS $$
BEGIN
    IF pg_trigger_depth() < 2 THEN
        RAISE EXCEPTION 'Artifact runtime ownership is captured only by its owning artifact write';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER artifact_runtime_owner_binding_guard
BEFORE INSERT OR UPDATE ON artifact_runtime_owner_binding
FOR EACH ROW EXECUTE FUNCTION guard_artifact_runtime_owner_binding();

CREATE FUNCTION capture_artifact_runtime_owner_binding() RETURNS TRIGGER AS $$
DECLARE
    structural_owner BIGINT;
    structural_pack BIGINT;
BEGIN
    -- Harmless metadata changes must not rebind an old artifact to a component
    -- that was deleted and recreated under the same ref.
    IF TG_OP = 'UPDATE' AND NEW.scope IS NOT DISTINCT FROM OLD.scope
       AND NEW.owner IS NOT DISTINCT FROM OLD.owner THEN
        RETURN NEW;
    END IF;
    CASE NEW.scope
        WHEN 'pack' THEN
            SELECT id, id INTO structural_owner, structural_pack FROM pack WHERE ref = NEW.owner;
        WHEN 'action' THEN
            SELECT id, CASE WHEN management_origin = 'pack' THEN pack ELSE NULL END
            INTO structural_owner, structural_pack FROM action WHERE ref = NEW.owner;
        WHEN 'sensor' THEN
            SELECT id, CASE WHEN management_origin = 'pack' THEN pack ELSE NULL END
            INTO structural_owner, structural_pack FROM sensor WHERE ref = NEW.owner;
        WHEN 'identity' THEN
            IF NEW.owner ~ '^[0-9]{1,19}$' THEN
                IF NEW.owner::NUMERIC BETWEEN 1 AND 9223372036854775807 THEN
                    SELECT id INTO structural_owner FROM identity WHERE id = NEW.owner::BIGINT;
                END IF;
            END IF;
        WHEN 'system' THEN
            structural_owner := NULL;
    END CASE;
    IF NEW.scope <> 'system' AND structural_owner IS NULL THEN
        DELETE FROM artifact_runtime_owner_binding WHERE artifact = NEW.id;
        RETURN NEW;
    END IF;
    INSERT INTO artifact_runtime_owner_binding
        (artifact, owner_type, owner_ref, owner_id, managing_pack)
    VALUES (NEW.id, NEW.scope, NEW.owner, structural_owner, structural_pack)
    ON CONFLICT (artifact) DO UPDATE SET
        owner_type = EXCLUDED.owner_type, owner_ref = EXCLUDED.owner_ref,
        owner_id = EXCLUDED.owner_id, managing_pack = EXCLUDED.managing_pack;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER artifact_capture_runtime_owner
AFTER INSERT OR UPDATE OF scope, owner ON artifact
FOR EACH ROW EXECUTE FUNCTION capture_artifact_runtime_owner_binding();

-- Do not backfill historical owner refs into trusted structural IDs. Existing
-- rows need explicit operator readmission; ordinary human artifact access is
-- unchanged. Owner IDs intentionally have no component/identity foreign keys.
