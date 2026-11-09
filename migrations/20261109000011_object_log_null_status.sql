-- Normalize absent operational metadata before comparing immutable identity.
-- Preserve PR31 per-stream delivery status after byte publication. These two
-- operational fields are not selected source/proof/body identity.
CREATE OR REPLACE FUNCTION guard_object_log_close_version() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF object_log_close_pinned(OLD.id) THEN RAISE EXCEPTION 'Object log close pins version'; END IF;
        RETURN OLD;
    END IF;
    IF EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
       AND (to_jsonb(NEW)-ARRAY['body_state','body_updated','legacy_snapshot_expires_at','size_bytes','sha256','provider_version','meta'])
           IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['body_state','body_updated','legacy_snapshot_expires_at','size_bytes','sha256','provider_version','meta']) THEN
        RAISE EXCEPTION 'Selected close forbids generic body mutation';
    END IF;
    IF OLD.body_state<>'pending' AND EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
       AND ROW(NEW.size_bytes,NEW.sha256,NEW.provider_version,COALESCE(NEW.meta,'{}'::jsonb)-ARRAY['log_state','log_failure'])
           IS DISTINCT FROM ROW(OLD.size_bytes,OLD.sha256,OLD.provider_version,COALESCE(OLD.meta,'{}'::jsonb)-ARRAY['log_state','log_failure']) THEN
        RAISE EXCEPTION 'Published selected result is immutable';
    END IF;
    -- Publication fields are checked against the selected input by deferred
    -- constraints on every participating table. No session bypass is available.
    IF EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
       AND ((COALESCE(NEW.meta,'{}'::jsonb)-ARRAY['encrypted_log_manifest','log_state','log_failure']) IS DISTINCT FROM
            (COALESCE(OLD.meta,'{}'::jsonb)-ARRAY['encrypted_log_manifest','log_state','log_failure'])) THEN
        RAISE EXCEPTION 'Selected close forbids metadata replacement';
    END IF;
    RETURN NEW;
END $$;
