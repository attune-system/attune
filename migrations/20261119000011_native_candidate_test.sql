-- Candidate tests retain install ownership and native effect evidence; no new scheduler.
ALTER TABLE execution_process_effect DROP CONSTRAINT execution_process_effect_purpose_check;
ALTER TABLE execution_process_effect ADD CONSTRAINT execution_process_effect_purpose_check
    CHECK (purpose IN ('action','runtime_probe','setup','candidate_test'));
ALTER TABLE execution_process_effect ADD COLUMN candidate_capture TEXT;
ALTER TABLE execution_process_effect ADD CONSTRAINT native_candidate_capture_shape CHECK
    (candidate_capture IS NULL OR (purpose='candidate_test' AND resource_removed AND exit_outcome IS NOT NULL AND stdin_consumed));
CREATE FUNCTION guard_native_candidate_capture() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.candidate_capture IS NOT NULL AND NEW.candidate_capture IS DISTINCT FROM OLD.candidate_capture THEN
        RAISE EXCEPTION 'Candidate capture is immutable';
    END IF;
    IF NEW.candidate_capture IS NOT NULL AND (NEW.purpose <> 'candidate_test' OR NOT NEW.resource_removed OR NEW.exit_outcome IS NULL OR NOT NEW.stdin_consumed) THEN
        RAISE EXCEPTION 'Candidate capture requires exact completed effects and removal';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER native_candidate_capture_guard BEFORE UPDATE ON execution_process_effect
FOR EACH ROW EXECUTE FUNCTION guard_native_candidate_capture();
CREATE FUNCTION revoke_native_candidate_install_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(OLD.status,OLD.assigned_worker_id,OLD.pack_id,OLD.candidate_object_key,OLD.candidate_provider_version,OLD.candidate_archive_size,OLD.candidate_digest,OLD.candidate_access_token_hash,OLD.started_at)
      IS DISTINCT FROM ROW(NEW.status,NEW.assigned_worker_id,NEW.pack_id,NEW.candidate_object_key,NEW.candidate_provider_version,NEW.candidate_archive_size,NEW.candidate_digest,NEW.candidate_access_token_hash,NEW.started_at) THEN
        PERFORM bump_workload_source_epoch('install',OLD.id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER native_candidate_install_epoch AFTER UPDATE OR DELETE ON pack_install
FOR EACH ROW EXECUTE FUNCTION revoke_native_candidate_install_epoch();
CREATE FUNCTION revoke_native_candidate_context_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(OLD.revoked,OLD.runtime_token_hash,OLD.expires_at,OLD.worker_id,OLD.archive_key,OLD.archive_version,OLD.archive_digest,OLD.archive_token_hash)
      IS DISTINCT FROM ROW(NEW.revoked,NEW.runtime_token_hash,NEW.expires_at,NEW.worker_id,NEW.archive_key,NEW.archive_version,NEW.archive_digest,NEW.archive_token_hash) THEN
        PERFORM bump_workload_source_epoch('install',OLD.install_id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER native_candidate_context_epoch AFTER UPDATE OR DELETE ON pack_candidate_context
FOR EACH ROW EXECUTE FUNCTION revoke_native_candidate_context_epoch();
