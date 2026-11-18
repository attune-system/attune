-- An observed native exit closes runtime authority before file inventory and
-- completion, while capture/custody may independently remain finalizing.
ALTER TABLE execution_process_effect ADD COLUMN exit_outcome JSONB;
ALTER TABLE execution_process_effect ADD CONSTRAINT execution_process_exit_shape CHECK (
    exit_outcome IS NULL OR (
        resource IS NOT NULL AND create_submitted
        AND jsonb_typeof(exit_outcome)='object'
        AND exit_outcome ?& ARRAY['exit_code','oom_killed']
        AND jsonb_typeof(exit_outcome->'exit_code')='number'
        AND (exit_outcome->>'exit_code') ~ '^-?[0-9]+$'
        AND (exit_outcome->>'exit_code')::BIGINT BETWEEN -2147483648 AND 2147483647
        AND jsonb_typeof(exit_outcome->'oom_killed')='boolean'
        AND exit_outcome-'exit_code'-'oom_killed'='{}'::JSONB
    )
);
-- Replacing the worker incarnation must not create another process for the
-- same execution attempt. The full target remains immutable ownership proof.
CREATE UNIQUE INDEX execution_process_one_attempt ON execution_process_effect
    (((target->>'execution')::BIGINT),(target->>'attempt'))
    WHERE target->>'kind'='execution';
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
       OR (OLD.exit_outcome IS NOT NULL AND NEW.exit_outcome IS DISTINCT FROM OLD.exit_outcome)
    THEN RAISE EXCEPTION 'Process identity and consumed effects are immutable'; END IF;
    RETURN NEW;
END $$;
