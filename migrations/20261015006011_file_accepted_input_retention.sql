-- A 01013 archives are internal ciphertext owners, never public SecretOrigins.
-- F keeps their exact rows/field copies while a physical file owns the proof.
DO $$ BEGIN
    IF to_regclass('runtime_accepted_input') IS NULL OR to_regclass('runtime_input_receipt_accepted') IS NULL THEN
        RAISE EXCEPTION 'Accepted-input file retention requires the committed A accepted-input seal migration';
    END IF;
END $$;

CREATE FUNCTION file_evidence_guard_accepted_archive() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE archive BIGINT;
BEGIN
    IF TG_TABLE_NAME='runtime_input_receipt_accepted' THEN
        IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Accepted receipt GC requires exclusive graph admission'; END IF;
        IF file_evidence_pinned('runtime_input_receipt',OLD.receipt) THEN RETURN NULL; END IF;
        RETURN OLD;
    ELSIF TG_TABLE_NAME='runtime_accepted_input' THEN archive:=OLD.id;
    ELSIF TG_TABLE_NAME='runtime_accepted_input_delivery' THEN archive:=OLD.accepted;
    ELSIF TG_TABLE_NAME='execution_secret_value' THEN
        IF OLD.entity_type<>'accepted_input' THEN
            IF TG_OP='UPDATE' THEN RETURN NEW; END IF;
            RETURN OLD;
        END IF;
        archive:=OLD.entity_id;
    END IF;
    IF TG_OP='UPDATE' THEN
        IF file_evidence_pinned('accepted_input',archive) THEN RAISE EXCEPTION 'Pinned accepted-input ciphertext is immutable'; END IF;
        RETURN NEW;
    END IF;
    IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Accepted archive GC requires exclusive graph admission'; END IF;
    IF file_evidence_pinned('accepted_input',archive) THEN RETURN NULL; END IF;
    RETURN OLD;
END; $$;

DO $$ DECLARE t TEXT; BEGIN
    FOREACH t IN ARRAY ARRAY['runtime_accepted_input','runtime_input_receipt_accepted','runtime_accepted_input_delivery'] LOOP
        EXECUTE format('CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON %I FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry()',t);
        EXECUTE format('CREATE TRIGGER file_evidence_accepted_guard BEFORE DELETE ON %I FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_accepted_archive()',t);
    END LOOP;
END $$;
-- Preserve the existing live replacement trigger and its other origin kinds.
CREATE TRIGGER file_evidence_accepted_guard BEFORE DELETE OR UPDATE ON execution_secret_value
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_accepted_archive();
