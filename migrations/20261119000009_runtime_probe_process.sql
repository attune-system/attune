-- Startup discovery owns a real retained admission, independently of actions.
-- No scheduler, execution authority or reusable prepared environment is created.
ALTER TABLE workload_source_epoch DROP CONSTRAINT workload_source_epoch_kind_check;
ALTER TABLE workload_source_epoch ADD CONSTRAINT workload_source_epoch_kind_check CHECK
    (kind IN ('owner','execution','worker','workload','sensor','admission','install','image','engine',
              'runtime_definition','runtime_version_definition','runtime_provider','runtime_probe_admission'));
CREATE TABLE runtime_probe_admission (
    id BIGSERIAL PRIMARY KEY,
    attempt UUID NOT NULL UNIQUE CHECK (attempt <> '00000000-0000-0000-0000-000000000000'::UUID),
    worker BIGINT NOT NULL CHECK (worker>0),
    incarnation UUID NOT NULL,
    runtime BIGINT NOT NULL CHECK (runtime>0),
    runtime_version BIGINT CHECK (runtime_version>0),
    provider_pack BIGINT NOT NULL CHECK (provider_pack>0),
    release BIGINT NOT NULL CHECK (release>0),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TRIGGER runtime_probe_admission_retain BEFORE DELETE ON runtime_probe_admission
FOR EACH ROW EXECUTE FUNCTION retain_workload_control_tombstone();
CREATE TRIGGER runtime_probe_admission_guard BEFORE UPDATE ON runtime_probe_admission
FOR EACH ROW EXECUTE FUNCTION guard_runtime_authority_record();
CREATE FUNCTION runtime_probe_source_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='runtime' THEN
        IF TG_OP='DELETE' OR ROW(NEW.ref,NEW.pack,NEW.execution_config,NEW.distributions,NEW.retired_at)
          IS DISTINCT FROM ROW(OLD.ref,OLD.pack,OLD.execution_config,OLD.distributions,OLD.retired_at) THEN
            PERFORM bump_workload_source_epoch('runtime_definition',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME='runtime_version' THEN
        IF TG_OP='DELETE' OR ROW(NEW.runtime,NEW.version,NEW.execution_config,NEW.distributions,NEW.retired_at)
          IS DISTINCT FROM ROW(OLD.runtime,OLD.version,OLD.execution_config,OLD.distributions,OLD.retired_at) THEN
            PERFORM bump_workload_source_epoch('runtime_version_definition',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME='pack' THEN
        IF TG_OP='DELETE' OR NEW.active_release IS DISTINCT FROM OLD.active_release THEN
            PERFORM bump_workload_source_epoch('runtime_provider',OLD.id);
        END IF;
    ELSE
        PERFORM bump_workload_source_epoch('runtime_probe_admission',OLD.id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER runtime_probe_runtime_epoch AFTER UPDATE OR DELETE ON runtime
FOR EACH ROW EXECUTE FUNCTION runtime_probe_source_epoch();
CREATE TRIGGER runtime_probe_version_epoch AFTER UPDATE OR DELETE ON runtime_version
FOR EACH ROW EXECUTE FUNCTION runtime_probe_source_epoch();
CREATE TRIGGER runtime_probe_provider_epoch AFTER UPDATE OR DELETE ON pack
FOR EACH ROW EXECUTE FUNCTION runtime_probe_source_epoch();
CREATE TRIGGER runtime_probe_admission_epoch AFTER UPDATE ON runtime_probe_admission
FOR EACH ROW EXECUTE FUNCTION runtime_probe_source_epoch();
ALTER TABLE execution_process_effect ADD COLUMN runtime_probe_tools JSONB
    CHECK (runtime_probe_tools IS NULL OR (purpose='runtime_probe' AND target->>'kind'='runtime_probe'
        AND jsonb_typeof(runtime_probe_tools)='object'));
-- The captured map is a monotonic receipt, never exit-only availability.
CREATE FUNCTION guard_runtime_probe_tools() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.runtime_probe_tools IS NOT NULL AND NEW.runtime_probe_tools IS DISTINCT FROM OLD.runtime_probe_tools THEN
        RAISE EXCEPTION 'Captured runtime tool evidence is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER runtime_probe_tools_guard BEFORE UPDATE ON execution_process_effect
FOR EACH ROW EXECUTE FUNCTION guard_runtime_probe_tools();
