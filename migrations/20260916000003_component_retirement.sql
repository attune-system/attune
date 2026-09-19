-- Pack projection lifecycle. Omitted definitions retain their identity and every
-- external relationship; only new-work resolution excludes retired rows.
DO $$
DECLARE component TEXT;
BEGIN
    FOREACH component IN ARRAY ARRAY[
        'runtime', 'permission_set', 'trigger', 'action', 'sensor', 'rule',
        'policy', 'work_queue', 'workflow_definition', 'dashboard', 'cache_namespace'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ADD COLUMN retired_at TIMESTAMPTZ', component);
        EXECUTE format('CREATE INDEX %I ON %I (retired_at) WHERE retired_at IS NULL',
            'idx_' || component || '_active', component);
    END LOOP;
END $$;

ALTER TABLE runtime_version ADD COLUMN retired_at TIMESTAMPTZ;
CREATE INDEX idx_runtime_version_active ON runtime_version (runtime, version)
    WHERE retired_at IS NULL;

-- Pack YAML owns `enabled`. Operators own this nullable override. Admission uses
-- COALESCE(enabled_override, enabled), so pack updates never erase an override.
DO $$
DECLARE component TEXT;
BEGIN
    FOREACH component IN ARRAY ARRAY[
        'trigger', 'action', 'sensor', 'rule', 'policy', 'work_queue', 'dashboard'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ADD COLUMN enabled_override BOOLEAN, ADD COLUMN effective_enabled BOOLEAN GENERATED ALWAYS AS (COALESCE(enabled_override, enabled)) STORED', component);
    END LOOP;
END $$;

-- A retired child cannot become selectable while its runtime remains retired.
CREATE OR REPLACE FUNCTION guard_runtime_version_lifecycle() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.retired_at IS NULL AND EXISTS (
        SELECT 1 FROM runtime WHERE id = NEW.runtime AND retired_at IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'active runtime version requires an active parent runtime';
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER guard_runtime_version_lifecycle
    BEFORE INSERT OR UPDATE OF runtime, retired_at ON runtime_version
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_version_lifecycle();
