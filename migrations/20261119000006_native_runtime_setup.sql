-- Native setup retains its existing coordinated installation owner/claim. The
-- build binding separates immutable image/platform/release/runtime/Runner pins;
-- it never hashes the selected private setup environment or stdin plan.
ALTER TABLE pack_environment ADD COLUMN native_binding TEXT
    CHECK (native_binding IS NULL OR native_binding ~ '^[0-9a-f]{64}$');
ALTER TABLE pack_environment DROP COLUMN env_key;
ALTER TABLE pack_environment ADD COLUMN env_key TEXT GENERATED ALWAYS AS (
    CASE WHEN native_binding IS NOT NULL THEN
        'native:' || pack::TEXT || ':' || runtime::TEXT || ':' || COALESCE(runtime_version::TEXT,'base') || ':' || native_binding
    WHEN runtime_version IS NULL THEN 'base:' || pack::TEXT || ':' || runtime::TEXT
    ELSE 'version:' || pack::TEXT || ':' || runtime::TEXT || ':' || runtime_version::TEXT END
) STORED;
ALTER TABLE pack_environment ADD UNIQUE(env_key);

-- Effect receipts outlive a changed/deleted claim. They are not installation
-- scheduling state, and a successful process exit alone is not ready authority.
ALTER TABLE execution_process_effect DROP CONSTRAINT execution_process_effect_purpose_check;
ALTER TABLE execution_process_effect ADD CONSTRAINT execution_process_effect_purpose_check
    CHECK (purpose IN ('action','runtime_probe','setup'));
ALTER TABLE execution_process_effect ADD COLUMN setup_binding TEXT
    CHECK (setup_binding IS NULL OR setup_binding ~ '^[0-9a-f]{64}$');
ALTER TABLE execution_process_effect ADD COLUMN published_runtime JSONB
    CHECK (published_runtime IS NULL OR
        (jsonb_typeof(published_runtime)='object' AND published_runtime->>'kind'='runtime'));
ALTER TABLE execution_process_effect ADD CONSTRAINT native_setup_receipt_shape
    CHECK ((purpose='setup' AND setup_binding IS NOT NULL)
        OR (purpose<>'setup' AND setup_binding IS NULL AND published_runtime IS NULL));
CREATE INDEX native_runtime_build_receipt ON execution_process_effect(setup_binding,id DESC)
    WHERE purpose='setup' AND published_runtime IS NOT NULL;
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
       OR (OLD.published_runtime IS NOT NULL AND NEW.published_runtime IS DISTINCT FROM OLD.published_runtime)
    THEN RAISE EXCEPTION 'Process identity and consumed effects are immutable'; END IF;
    RETURN NEW;
END $$;
