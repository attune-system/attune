-- Grouping is based on descriptive enqueue-time attribution, not a live IAM
-- relationship. Identity deletion may clear requested_by_identity but must not
-- merge batches that were explicitly separated by requester.
ALTER TABLE work_queue_item ADD COLUMN historical_requester_identity BIGINT
    CHECK (historical_requester_identity IS NULL OR historical_requester_identity > 0);

-- Unknown historical requesters remain unknown. Never guess from payloads,
-- metadata, a queue owner or a dispatch execution.
UPDATE work_queue_item SET historical_requester_identity = requested_by_identity;

CREATE FUNCTION guard_queue_requester_grouping_attribution() RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.historical_requester_identity := NEW.requested_by_identity;
    ELSIF NEW.historical_requester_identity IS DISTINCT FROM OLD.historical_requester_identity THEN
        RAISE EXCEPTION 'Queue requester grouping attribution is immutable';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER guard_queue_requester_grouping_attribution
    BEFORE INSERT OR UPDATE ON work_queue_item
    FOR EACH ROW EXECUTE FUNCTION guard_queue_requester_grouping_attribution();
