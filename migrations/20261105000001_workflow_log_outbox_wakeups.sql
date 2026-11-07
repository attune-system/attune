-- Notifications are empty, coalescible wakeup hints. Claims remain authoritative.
CREATE FUNCTION notify_workflow_log_outbox_ready() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.is_head AND NEW.delivered_at IS NULL AND NEW.failed_at IS NULL THEN
        IF TG_OP = 'INSERT' THEN
            PERFORM pg_notify('workflow_log_outbox_ready', '');
        ELSIF NOT OLD.is_head
           OR OLD.failed_at IS DISTINCT FROM NEW.failed_at
           OR OLD.available_at IS DISTINCT FROM NEW.available_at
           OR (OLD.claimed_by IS NOT NULL AND NEW.claimed_by IS NULL)
           OR (OLD.claim_expires_at IS NOT NULL AND (NEW.claim_expires_at IS NULL OR NEW.claim_expires_at < OLD.claim_expires_at)) THEN
            PERFORM pg_notify('workflow_log_outbox_ready', '');
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER workflow_log_outbox_ready_notify
AFTER INSERT OR UPDATE OF is_head, delivered_at, failed_at, available_at, claimed_by, claim_expires_at
ON workflow_log_outbox
FOR EACH ROW EXECUTE FUNCTION notify_workflow_log_outbox_ready();
