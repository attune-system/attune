-- Existing intents may already have an unacknowledged native create. Conservatively
-- reconcile them; absence of resource evidence is never proof of no submission.
ALTER TABLE execution_process_effect ADD COLUMN create_submitted BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE execution_process_effect ALTER COLUMN create_submitted SET DEFAULT FALSE;
ALTER TABLE execution_process_effect ADD CONSTRAINT execution_process_resource_requires_submit
    CHECK (resource IS NULL OR create_submitted);
ALTER TABLE execution_process_effect ADD CONSTRAINT execution_process_input_requires_resource
    CHECK (NOT stdin_consumed OR (create_submitted AND resource IS NOT NULL));

CREATE OR REPLACE FUNCTION guard_execution_process_effect() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'Process effect evidence requires explicit owned reclamation';
    END IF;
    IF ROW(NEW.id,NEW.intent,NEW.target,NEW.encrypted_context,NEW.source_epochs,NEW.created)
       IS DISTINCT FROM
       ROW(OLD.id,OLD.intent,OLD.target,OLD.encrypted_context,OLD.source_epochs,OLD.created)
       OR (OLD.resource IS NOT NULL AND NEW.resource IS DISTINCT FROM OLD.resource)
       OR (OLD.stdin_consumed AND NOT NEW.stdin_consumed)
       OR (OLD.stop_requested AND NOT NEW.stop_requested)
       OR (OLD.create_submitted AND NOT NEW.create_submitted)
    THEN RAISE EXCEPTION 'Process identity and consumed effects are immutable'; END IF;
    RETURN NEW;
END $$;
