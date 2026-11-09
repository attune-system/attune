-- Expiry retires authority without changing identity or claiming PUT settlement.
-- Maintenance already owns the grant row; it must not acquire the stream in the
-- opposite order to producer admission (stream, then grant).
CREATE OR REPLACE FUNCTION guard_object_log_close_grant() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE s log_stream;
BEGIN
    IF TG_OP='UPDATE' THEN
        IF OLD.state='issued' AND NEW.state='expired'
           AND OLD.settle_until<=clock_timestamp()
           AND (to_jsonb(NEW)-ARRAY['state','updated']) IS NOT DISTINCT FROM
               (to_jsonb(OLD)-ARRAY['state','updated']) THEN
            RETURN NEW;
        END IF;
    END IF;
    SELECT * INTO s FROM log_stream WHERE artifact_version=NEW.artifact_version FOR UPDATE;
    IF s.close_phase IS NOT NULL THEN
        -- Only a no-op completed retry for the exact accepted provider object.
        IF NEW.state<>'completed' OR NOT EXISTS(SELECT 1 FROM log_segment g WHERE g.stream=s.id
            AND g.sequence=NEW.segment_sequence AND g.object_key=NEW.object_key
            AND g.size_bytes=NEW.expected_size AND g.sha256=NEW.expected_sha256
            AND g.provider_version=NEW.completed_provider_version) THEN
            RAISE EXCEPTION 'Object close forbids upload admission or changed completion';
        END IF;
    END IF;
    RETURN NEW;
END $$;
