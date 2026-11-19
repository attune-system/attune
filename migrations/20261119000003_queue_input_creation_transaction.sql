-- A row version's xmin does not prove original creation and changes across
-- savepoints. Historical items intentionally receive no creation evidence.
ALTER TABLE work_queue_item ADD COLUMN input_creation_transaction xid8;
ALTER TABLE work_queue_item ALTER COLUMN input_creation_transaction SET DEFAULT pg_current_xact_id();

CREATE FUNCTION protect_queue_item_input_creation_transaction() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.input_creation_transaction IS DISTINCT FROM OLD.input_creation_transaction THEN
        RAISE EXCEPTION 'Queue item input creation transaction is immutable';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER protect_queue_item_input_creation_transaction
    BEFORE UPDATE OF input_creation_transaction ON work_queue_item FOR EACH ROW
    EXECUTE FUNCTION protect_queue_item_input_creation_transaction();
