-- Source trace tags for event and queue-item origin records.
--
-- These tags allow execution-token initiated event/queue-item creation to
-- preserve an existing trace when no explicit rule/queue template overrides it.

ALTER TABLE event
    ADD COLUMN trace_tag TEXT;

ALTER TABLE work_queue_item
    ADD COLUMN trace_tag TEXT;

CREATE INDEX idx_event_trace_tag
    ON event(trace_tag)
    WHERE trace_tag IS NOT NULL;

CREATE INDEX idx_work_queue_item_trace_tag
    ON work_queue_item(trace_tag)
    WHERE trace_tag IS NOT NULL;

COMMENT ON COLUMN event.trace_tag IS
    'Optional source trace tag attached at event creation (explicit request or execution-token inheritance). Used as fallback when rule trace_tag_template is unset.';

COMMENT ON COLUMN work_queue_item.trace_tag IS
    'Optional source trace tag attached at enqueue time (explicit request or execution-token inheritance). Used as fallback when queue trace_tag_template is unset.';
