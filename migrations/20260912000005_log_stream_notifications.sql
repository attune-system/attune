CREATE OR REPLACE FUNCTION notify_log_stream_changed()
RETURNS TRIGGER AS $$
BEGIN
    PERFORM pg_notify(
        'log_stream_changed',
        json_build_object(
            'stream_id', NEW.id,
            'artifact_version_id', NEW.artifact_version,
            'total_bytes', NEW.total_bytes,
            'sealed', NEW.sealed
        )::text
    );
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER log_stream_changed_notify
    AFTER UPDATE OF total_bytes, sealed ON log_stream
    FOR EACH ROW
    WHEN (OLD.total_bytes IS DISTINCT FROM NEW.total_bytes OR OLD.sealed IS DISTINCT FROM NEW.sealed)
    EXECUTE FUNCTION notify_log_stream_changed();

COMMENT ON FUNCTION notify_log_stream_changed() IS
    'Emits one compact, transaction-bound wakeup after log bytes commit or a stream seals.';
