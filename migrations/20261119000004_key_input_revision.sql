-- Internal non-value version identity for admitted queue input cohorts.
-- Every update advances it, including repeated updates in one transaction.
ALTER TABLE key ADD COLUMN input_revision BIGINT NOT NULL DEFAULT 1 CHECK (input_revision > 0);
CREATE FUNCTION advance_key_input_revision() RETURNS TRIGGER AS $$
BEGIN
    -- Ignore any supplied revision; overflow fails the whole UPDATE atomically.
    NEW.input_revision := OLD.input_revision + 1;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER advance_key_input_revision BEFORE UPDATE ON key FOR EACH ROW
    EXECUTE FUNCTION advance_key_input_revision();
