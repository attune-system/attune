-- Until dispatch consumes an immutable accepted template revision, all writers
-- must drain accepted items before changing the behavior they would observe.
CREATE FUNCTION guard_accepted_queue_templates() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.action_params IS DISTINCT FROM NEW.action_params
        OR OLD.dispatch_action IS DISTINCT FROM NEW.dispatch_action
        OR OLD.dispatch_action_ref IS DISTINCT FROM NEW.dispatch_action_ref
        OR OLD.permission_set_refs IS DISTINCT FROM NEW.permission_set_refs
        OR OLD.item_schema IS DISTINCT FROM NEW.item_schema
        OR OLD.trace_tag_template IS DISTINCT FROM NEW.trace_tag_template
        OR OLD.config IS DISTINCT FROM NEW.config THEN
        IF EXISTS (SELECT 1 FROM work_queue_item WHERE queue = OLD.id AND status IN ('queued','leased','retry')) THEN
            RAISE EXCEPTION 'Drain accepted queue items before changing dispatch templates or policy';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_work_queue_accepted_templates BEFORE UPDATE ON work_queue
    FOR EACH ROW EXECUTE FUNCTION guard_accepted_queue_templates();
