-- xmin changes on UPDATE, so it cannot prove that an execution was INSERTed in
-- the effect transaction. Historical rows deliberately receive no birth token.
ALTER TABLE execution ADD COLUMN initial_input_transaction xid8;
ALTER TABLE execution ALTER COLUMN initial_input_transaction SET DEFAULT pg_current_xact_id();

CREATE FUNCTION protect_execution_initial_input_transaction() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.initial_input_transaction IS DISTINCT FROM OLD.initial_input_transaction THEN
        RAISE EXCEPTION 'Execution initial input transaction is immutable';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER protect_execution_initial_input_transaction
    BEFORE UPDATE OF initial_input_transaction ON execution FOR EACH ROW
    EXECUTE FUNCTION protect_execution_initial_input_transaction();
