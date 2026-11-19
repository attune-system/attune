-- Positive native cleanup evidence, not a new lifecycle/scheduling state.
-- Retain this receipt independently of mutable installation claims.
ALTER TABLE execution_process_effect ADD COLUMN resource_removed BOOLEAN NOT NULL DEFAULT FALSE;
CREATE OR REPLACE FUNCTION guard_execution_process_effect() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'Process effect evidence requires explicit owned reclamation';
    END IF;
    IF ROW(NEW.id,NEW.intent,NEW.target,NEW.purpose,NEW.encrypted_context,NEW.source_epochs,NEW.setup_binding,NEW.created)
       IS DISTINCT FROM
       ROW(OLD.id,OLD.intent,OLD.target,OLD.purpose,OLD.encrypted_context,OLD.source_epochs,OLD.setup_binding,OLD.created)
       OR (OLD.resource IS NOT NULL AND NEW.resource IS DISTINCT FROM OLD.resource)
       OR (OLD.stdin_consumed AND NOT NEW.stdin_consumed)
       OR (OLD.stop_requested AND NOT NEW.stop_requested)
       OR (OLD.create_submitted AND NOT NEW.create_submitted)
       OR (OLD.exit_outcome IS NOT NULL AND NEW.exit_outcome IS DISTINCT FROM OLD.exit_outcome)
       OR (OLD.resource_removed AND NOT NEW.resource_removed)
       OR (OLD.published_runtime IS NOT NULL AND NEW.published_runtime IS DISTINCT FROM OLD.published_runtime)
    THEN RAISE EXCEPTION 'Process identity and consumed effects are immutable'; END IF;
    RETURN NEW;
END $$;
