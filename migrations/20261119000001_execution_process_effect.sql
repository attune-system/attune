-- Native provider effects owned by the existing trusted execution loop.
-- No host enrollment, dispatch bearer, lease scheduler or second domain state.
-- Evidence intentionally survives independently retained execution rows.
CREATE TABLE execution_process_effect (
    id BIGSERIAL PRIMARY KEY,
    intent UUID NOT NULL UNIQUE,
    target JSONB NOT NULL CHECK (jsonb_typeof(target)='object'),
    encrypted_context TEXT NOT NULL,
    source_epochs JSONB NOT NULL CHECK (jsonb_typeof(source_epochs)='array'),
    resource JSONB CHECK (resource IS NULL OR jsonb_typeof(resource)='object'),
    stdin_consumed BOOLEAN NOT NULL DEFAULT FALSE,
    stop_requested BOOLEAN NOT NULL DEFAULT FALSE,
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(target),
    CHECK (intent <> '00000000-0000-0000-0000-000000000000'::UUID)
);
CREATE FUNCTION guard_execution_process_effect() RETURNS trigger LANGUAGE plpgsql AS $$
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
    THEN RAISE EXCEPTION 'Process identity and consumed effects are immutable'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER execution_process_effect_guard BEFORE UPDATE OR DELETE ON execution_process_effect
FOR EACH ROW EXECUTE FUNCTION guard_execution_process_effect();
