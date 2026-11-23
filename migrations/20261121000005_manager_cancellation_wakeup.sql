CREATE FUNCTION notify_manager_execution_cancellation() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    session_id BIGINT;
BEGIN
    SELECT manager_session INTO session_id FROM manager_execution_assignment
    WHERE execution = NEW.id AND closed_at IS NULL AND claim_request IS NOT NULL;
    IF session_id IS NOT NULL THEN
        PERFORM pg_notify('manager_work_available', session_id::text);
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER manager_execution_cancellation_wakeup
AFTER UPDATE OF status ON execution FOR EACH ROW
WHEN (NEW.status = 'canceling' AND OLD.status IS DISTINCT FROM NEW.status)
EXECUTE FUNCTION notify_manager_execution_cancellation();
