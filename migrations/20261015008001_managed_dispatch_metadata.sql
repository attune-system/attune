-- Managed API dispatch edits retain source metadata under their immutable policy.
-- Policy deletion reclaims these records; old admissions already pin that policy.
CREATE TABLE managed_dispatch_metadata_revision (
    policy_revision BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE CASCADE,
    component_kind TEXT NOT NULL CHECK (component_kind IN ('rules','queues')),
    component_id BIGINT NOT NULL,
    source_release BIGINT NOT NULL REFERENCES pack_release(id) DEFERRABLE INITIALLY DEFERRED,
    metadata JSONB NOT NULL,
    metadata_digest TEXT NOT NULL,
    PRIMARY KEY (policy_revision,component_kind,component_id)
);

CREATE FUNCTION guard_managed_dispatch_metadata() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Managed dispatch metadata revisions are immutable';
END;
$$;
CREATE TRIGGER managed_dispatch_metadata_immutable BEFORE UPDATE ON managed_dispatch_metadata_revision
    FOR EACH ROW EXECUTE FUNCTION guard_managed_dispatch_metadata();

-- Retain A's existing guard, including the batch envelope that accepted payloads
-- would otherwise be reinterpreted under. Labels and enqueue-only defaults stay
-- outside this gate.
CREATE OR REPLACE FUNCTION guard_accepted_queue_templates() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.action_params IS DISTINCT FROM NEW.action_params
        OR OLD.dispatch_action IS DISTINCT FROM NEW.dispatch_action
        OR OLD.dispatch_action_ref IS DISTINCT FROM NEW.dispatch_action_ref
        OR OLD.permission_set_refs IS DISTINCT FROM NEW.permission_set_refs
        OR OLD.item_schema IS DISTINCT FROM NEW.item_schema
        OR OLD.trace_tag_template IS DISTINCT FROM NEW.trace_tag_template
        OR OLD.config IS DISTINCT FROM NEW.config
        OR OLD.batch_mode IS DISTINCT FROM NEW.batch_mode THEN
        IF EXISTS (SELECT 1 FROM work_queue_item WHERE queue = OLD.id AND status IN ('queued','leased','retry')) THEN
            RAISE EXCEPTION 'Drain accepted queue items before changing dispatch templates or policy';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
