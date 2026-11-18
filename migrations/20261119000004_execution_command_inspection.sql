-- Distinguish the fixed read-only command inspection from its owning action.
-- These are native effect receipts, not scheduling or domain lifecycle state.
ALTER TABLE execution_process_effect ADD COLUMN purpose TEXT NOT NULL DEFAULT 'action'
    CHECK (purpose IN ('action','runtime_probe'));
ALTER TABLE execution_process_effect DROP CONSTRAINT execution_process_effect_target_key;
ALTER TABLE execution_process_effect ADD CONSTRAINT execution_process_target_purpose UNIQUE(target,purpose);
DROP INDEX execution_process_one_attempt;
CREATE UNIQUE INDEX execution_process_one_attempt ON execution_process_effect
    (((target->>'execution')::BIGINT),(target->>'attempt'),purpose)
    WHERE target->>'kind'='execution';
-- Program capture remains independently reclaimed with its existing execution
-- metadata. No new FK couples retained provider evidence to this optional proof.
ALTER TABLE execution_program_selection ADD COLUMN native_probe UUID;
CREATE OR REPLACE FUNCTION guard_execution_process_effect() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'Process effect evidence requires explicit owned reclamation';
    END IF;
    IF ROW(NEW.id,NEW.intent,NEW.target,NEW.purpose,NEW.encrypted_context,NEW.source_epochs,NEW.created)
       IS DISTINCT FROM
       ROW(OLD.id,OLD.intent,OLD.target,OLD.purpose,OLD.encrypted_context,OLD.source_epochs,OLD.created)
       OR (OLD.resource IS NOT NULL AND NEW.resource IS DISTINCT FROM OLD.resource)
       OR (OLD.stdin_consumed AND NOT NEW.stdin_consumed)
       OR (OLD.stop_requested AND NOT NEW.stop_requested)
       OR (OLD.create_submitted AND NOT NEW.create_submitted)
       OR (OLD.exit_outcome IS NOT NULL AND NEW.exit_outcome IS DISTINCT FROM OLD.exit_outcome)
    THEN RAISE EXCEPTION 'Process identity and consumed effects are immutable'; END IF;
    RETURN NEW;
END $$;
