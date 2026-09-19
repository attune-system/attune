CREATE TYPE absent_metadata_policy AS ENUM ('remove', 'disable', 'retain');

ALTER TABLE pack_install
    ADD COLUMN absent_metadata_policy absent_metadata_policy NOT NULL DEFAULT 'remove';

ALTER TABLE runtime_version
    ADD COLUMN managed_release BIGINT REFERENCES pack_release(id)
        DEFERRABLE INITIALLY DEFERRED;

UPDATE runtime_version rv
SET managed_release = r.managed_release
FROM runtime r
WHERE r.id = rv.runtime AND r.management_origin = 'pack';

CREATE FUNCTION guard_runtime_version_managed_release() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.managed_release IS NOT NULL AND NOT EXISTS (
        SELECT 1
        FROM runtime r
        JOIN pack_release pr ON pr.id = NEW.managed_release AND pr.pack = r.pack
        WHERE r.id = NEW.runtime AND r.management_origin = 'pack'
    ) THEN
        RAISE EXCEPTION 'runtime version managed release must belong to its runtime pack';
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER guard_runtime_version_managed_release
    BEFORE INSERT OR UPDATE OF runtime, managed_release ON runtime_version
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_version_managed_release();

-- Omission is an independent admission gate. Pack declarations and operator
-- overrides retain their existing ownership and become effective again when
-- the component reappears.
DO $$
DECLARE component TEXT;
BEGIN
    FOREACH component IN ARRAY ARRAY[
        'trigger', 'action', 'sensor', 'rule', 'policy', 'work_queue', 'dashboard'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ADD COLUMN omission_disabled BOOLEAN NOT NULL DEFAULT FALSE', component);
        EXECUTE format('ALTER TABLE %I DROP COLUMN effective_enabled', component);
        EXECUTE format('ALTER TABLE %I ADD COLUMN effective_enabled BOOLEAN GENERATED ALWAYS AS (NOT omission_disabled AND COALESCE(enabled_override, enabled)) STORED', component);
    END LOOP;
END $$;

CREATE TABLE pack_release_executable (
    release BIGINT NOT NULL REFERENCES pack_release(id) ON DELETE CASCADE,
    component_kind TEXT NOT NULL CHECK (component_kind IN ('action', 'sensor')),
    component_id BIGINT NOT NULL,
    component_ref TEXT NOT NULL,
    snapshot JSONB NOT NULL CHECK (jsonb_typeof(snapshot) = 'object'),
    PRIMARY KEY (release, component_kind, component_id),
    UNIQUE (release, component_kind, component_ref)
);

CREATE INDEX idx_pack_release_executable_component
    ON pack_release_executable(component_kind, component_id, release);

INSERT INTO pack_release_executable
    (release, component_kind, component_id, component_ref, snapshot)
SELECT a.managed_release, 'action', a.id, a.ref, jsonb_build_object(
    'action', to_jsonb(a),
    'runtime', to_jsonb(r),
    'runtime_versions', COALESCE((
        SELECT jsonb_agg(to_jsonb(rv) ORDER BY rv.version, rv.id)
        FROM runtime_version rv
        WHERE rv.runtime = r.id AND rv.retired_at IS NULL
    ), '[]'::jsonb),
    'workflow_definition', to_jsonb(wd)
)
FROM action a
LEFT JOIN runtime r ON r.id = a.runtime AND r.retired_at IS NULL
LEFT JOIN workflow_definition wd ON wd.id = a.workflow_def AND wd.retired_at IS NULL
WHERE a.managed_release IS NOT NULL AND a.retired_at IS NULL;

INSERT INTO pack_release_executable
    (release, component_kind, component_id, component_ref, snapshot)
SELECT s.managed_release, 'sensor', s.id, s.ref, jsonb_build_object(
    'sensor', to_jsonb(s),
    'runtime', to_jsonb(r),
    'runtime_versions', COALESCE((
        SELECT jsonb_agg(to_jsonb(rv) ORDER BY rv.version, rv.id)
        FROM runtime_version rv
        WHERE rv.runtime = r.id AND rv.retired_at IS NULL
    ), '[]'::jsonb)
)
FROM sensor s
JOIN runtime r ON r.id = s.runtime AND r.retired_at IS NULL
WHERE s.managed_release IS NOT NULL AND s.retired_at IS NULL;

-- Retired rows preserve identity and relationships, not executable content.
-- Clearing old provenance keeps bounded release retention possible.
DO $$
DECLARE
    component TEXT;
BEGIN
    FOREACH component IN ARRAY ARRAY[
        'runtime', 'permission_set', 'trigger', 'action', 'sensor', 'rule',
        'policy', 'work_queue', 'workflow_definition', 'dashboard', 'cache_namespace'
    ] LOOP
        EXECUTE format('UPDATE %I SET managed_release = NULL WHERE retired_at IS NOT NULL', component);
    END LOOP;
END $$;
