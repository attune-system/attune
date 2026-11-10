-- Generic object-log seal/Ready must not bypass the durable protocol. Shared-file
-- callers retain their existing snapshot/flock publication path.
CREATE FUNCTION require_object_log_close_selection() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='log_stream' THEN
        IF NEW.backend='object_segments' AND NEW.sealed AND NOT OLD.sealed
           AND NEW.close_phase IS DISTINCT FROM 'published' THEN
            RAISE EXCEPTION 'Object log publication requires selected verified close';
        END IF;
    ELSIF NEW.body_state='ready' AND OLD.body_state IS DISTINCT FROM NEW.body_state
          AND EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=NEW.id
                     AND backend='object_segments' AND close_phase IS DISTINCT FROM 'published') THEN
        RAISE EXCEPTION 'Object log Ready requires selected verified close';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER object_log_selection_before_seal BEFORE UPDATE ON log_stream
    FOR EACH ROW EXECUTE FUNCTION require_object_log_close_selection();
CREATE TRIGGER object_log_selection_before_ready BEFORE UPDATE ON artifact_version
    FOR EACH ROW EXECUTE FUNCTION require_object_log_close_selection();
