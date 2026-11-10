CREATE FUNCTION guard_object_log_publication_identity() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='log_stream' THEN
        IF ROW(NEW.backend,NEW.artifact_version,NEW.id) IS DISTINCT FROM
           ROW(OLD.backend,OLD.artifact_version,OLD.id) THEN
            RAISE EXCEPTION 'Log stream binding and backend are immutable';
        END IF;
    ELSIF EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
          AND ROW(NEW.size_bytes,NEW.sha256,NEW.provider_version) IS DISTINCT FROM
              ROW(OLD.size_bytes,OLD.sha256,OLD.provider_version)
          AND NOT (OLD.body_state='pending' AND NEW.body_state='ready' AND EXISTS(
              SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase='published')) THEN
        RAISE EXCEPTION 'Selected bytes may change only in atomic publication';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER object_log_publication_identity_stream BEFORE UPDATE ON log_stream
    FOR EACH ROW EXECUTE FUNCTION guard_object_log_publication_identity();
CREATE TRIGGER object_log_publication_identity_version BEFORE UPDATE ON artifact_version
    FOR EACH ROW EXECUTE FUNCTION guard_object_log_publication_identity();
