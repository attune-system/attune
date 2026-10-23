-- Defaults and overrides point at immutable, secret-safe configuration revisions.
-- Historical effective values are conservatively treated as overrides on first use.
CREATE TABLE component_configuration_layers (
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('pack', 'sensor')),
    owner_id BIGINT NOT NULL CHECK (owner_id > 0),
    defaults_revision BIGINT NOT NULL REFERENCES component_configuration_revision(id),
    override_fields TEXT[] NOT NULL DEFAULT '{}',
    PRIMARY KEY (owner_kind, owner_id),
    FOREIGN KEY (owner_kind, owner_id) REFERENCES component_configuration_current(owner_kind, owner_id)
        ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX component_configuration_layers_defaults_idx
    ON component_configuration_layers(defaults_revision);

CREATE FUNCTION check_configuration_layers_owner() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM component_configuration_revision
        WHERE id = NEW.defaults_revision AND owner_kind = NEW.owner_kind AND owner_id = NEW.owner_id) THEN
        RAISE EXCEPTION 'configuration defaults ownership mismatch';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER check_configuration_layers_owner BEFORE INSERT OR UPDATE ON component_configuration_layers
    FOR EACH ROW EXECUTE FUNCTION check_configuration_layers_owner();
