-- Managed batch manifests retain their exact fixed-input record independently
-- of source item retention. Action/ad-hoc manifests have no queue input binding.
ALTER TABLE runtime_accepted_batch_item ADD COLUMN configuration_input BIGINT
    REFERENCES work_queue_item_configuration_input(id) ON DELETE RESTRICT;
CREATE INDEX runtime_accepted_batch_configuration_input ON runtime_accepted_batch_item(configuration_input)
    WHERE configuration_input IS NOT NULL;

CREATE FUNCTION guard_accepted_batch_evidence_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT file_evidence_graph_held(true) THEN
        RAISE EXCEPTION 'Accepted batch reclamation requires exclusive graph admission';
    END IF;
    IF file_evidence_pinned('runtime_authority_admission',OLD.admission) THEN RETURN NULL; END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON runtime_accepted_batch_item
    FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry();
CREATE TRIGGER file_evidence_guard BEFORE DELETE ON runtime_accepted_batch_item
    FOR EACH ROW EXECUTE FUNCTION guard_accepted_batch_evidence_delete();
CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON runtime_accepted_batch_manifest
    FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry();
CREATE TRIGGER file_evidence_guard BEFORE DELETE ON runtime_accepted_batch_manifest
    FOR EACH ROW EXECUTE FUNCTION guard_accepted_batch_evidence_delete();
