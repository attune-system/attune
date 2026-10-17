-- Same-advance values are canonical renders, not arbitrary prepared JSON. Their
-- original publication and every downstream receipt share the preparation TX.
CREATE TABLE runtime_input_receipt_staged (
    receipt BIGINT PRIMARY KEY REFERENCES runtime_input_receipt(id) ON DELETE CASCADE,
    prepared_transaction TEXT NOT NULL DEFAULT pg_current_xact_id()::TEXT,
    publication_transaction TEXT,
    CHECK (publication_transaction IS NULL OR publication_transaction = prepared_transaction)
);

CREATE FUNCTION protect_staged_input_publication() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.prepared_transaction <> pg_current_xact_id()::TEXT OR NEW.publication_transaction IS NOT NULL THEN
            RAISE EXCEPTION 'Staged input must start unpublished in its preparation transaction';
        END IF;
    ELSIF NEW.receipt IS DISTINCT FROM OLD.receipt
        OR NEW.prepared_transaction IS DISTINCT FROM OLD.prepared_transaction
        OR OLD.publication_transaction IS NOT NULL
        OR NEW.publication_transaction IS DISTINCT FROM pg_current_xact_id()::TEXT
        OR NEW.prepared_transaction <> pg_current_xact_id()::TEXT THEN
        RAISE EXCEPTION 'Staged input publication is immutable and transaction bound';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER protect_staged_input_publication
BEFORE INSERT OR UPDATE ON runtime_input_receipt_staged
FOR EACH ROW EXECUTE FUNCTION protect_staged_input_publication();

CREATE FUNCTION protect_staged_input_publication_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM runtime_input_receipt WHERE id=OLD.receipt) THEN
        RAISE EXCEPTION 'Staged input publication dependency is immutable';
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER protect_staged_input_publication_delete
BEFORE DELETE ON runtime_input_receipt_staged
FOR EACH ROW EXECUTE FUNCTION protect_staged_input_publication_delete();
