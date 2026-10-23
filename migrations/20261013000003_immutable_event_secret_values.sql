-- Immutable events include their encrypted fields. Physical partition repair
-- preserves these rows; owner deletion/expiry permits their cleanup afterward.
CREATE FUNCTION guard_event_secret_values() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.entity_type IN ('event_payload', 'event_config')
       AND EXISTS (SELECT 1 FROM event WHERE id = OLD.entity_id) THEN
        RAISE EXCEPTION 'event secret values are immutable';
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_event_secret_values BEFORE UPDATE OR DELETE ON execution_secret_value
    FOR EACH ROW EXECUTE FUNCTION guard_event_secret_values();
