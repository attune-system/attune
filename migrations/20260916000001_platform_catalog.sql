-- Coordinated maintenance migration. Stop old writers and revoke their database
-- credentials before applying this migration. The epoch is not an old-binary fence.
CREATE TYPE management_origin AS ENUM ('platform', 'pack', 'ad_hoc');

CREATE TABLE platform_catalog_state (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    compatibility_epoch INTEGER NOT NULL CHECK (compatibility_epoch > 0),
    revision INTEGER NOT NULL CHECK (revision >= 0)
);
INSERT INTO platform_catalog_state (compatibility_epoch, revision) VALUES (1, 0);

CREATE TABLE intrinsic_handler (
    ref TEXT PRIMARY KEY,
    catalog_revision INTEGER NOT NULL CHECK (catalog_revision > 0),
    allowed_component_refs TEXT[] NOT NULL,
    param_schema JSONB NOT NULL,
    out_schema JSONB NOT NULL
);

-- Ownership is stored, but cannot disagree with the existing owner and ad-hoc
-- fields. Associated pack IDs on ad-hoc components are not lifecycle ownership.
DO $$
DECLARE
    component TEXT;
    owner_column TEXT;
    adhoc_expression TEXT;
BEGIN
    FOREACH component IN ARRAY ARRAY[
        'runtime', 'permission_set', 'trigger', 'action', 'sensor', 'rule',
        'policy', 'work_queue', 'workflow_definition', 'dashboard', 'cache_namespace'
    ] LOOP
        owner_column := CASE WHEN component = 'cache_namespace' THEN 'managing_pack' ELSE 'pack' END;
        adhoc_expression := CASE WHEN component IN (
            'trigger', 'action', 'sensor', 'rule', 'work_queue', 'dashboard'
        ) THEN 'is_adhoc OR ' ELSE '' END;
        EXECUTE format('ALTER TABLE %I
            ADD COLUMN catalog_revision INTEGER CHECK (catalog_revision > 0),
            ADD COLUMN managed_release BIGINT,
            ADD COLUMN management_origin management_origin GENERATED ALWAYS AS (
                CASE WHEN catalog_revision IS NOT NULL THEN ''platform''::management_origin
                     WHEN %s%I IS NULL THEN ''ad_hoc''::management_origin
                     ELSE ''pack''::management_origin END
            ) STORED,
            ADD CONSTRAINT %I CHECK (catalog_revision IS NULL OR (%I IS NULL AND NOT (%sFALSE)))',
            component, adhoc_expression, owner_column,
            component || '_platform_owner', owner_column, adhoc_expression);
        EXECUTE format('ALTER TABLE %I
            ADD CONSTRAINT %I FOREIGN KEY (%I, managed_release) REFERENCES pack_release(pack, id) DEFERRABLE INITIALLY DEFERRED,
            ADD CONSTRAINT %I CHECK (management_origin = ''pack'' OR managed_release IS NULL)',
            component, component || '_managed_release_owner', owner_column, component || '_managed_release_origin');
        EXECUTE format('UPDATE %I c SET managed_release = p.active_release FROM pack p
            WHERE c.%I = p.id AND c.management_origin = ''pack''', component, owner_column);
    END LOOP;
END $$;

CREATE FUNCTION set_component_managed_release() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE owner_id BIGINT;
BEGIN
    owner_id := (to_jsonb(NEW)->>TG_ARGV[0])::BIGINT;
    IF NEW.catalog_revision IS NOT NULL OR owner_id IS NULL OR
       COALESCE((to_jsonb(NEW)->>'is_adhoc')::BOOLEAN, FALSE) THEN
        NEW.managed_release := NULL;
    ELSIF TG_OP = 'INSERT' OR
          (to_jsonb(NEW)->TG_ARGV[0]) IS DISTINCT FROM (to_jsonb(OLD)->TG_ARGV[0]) THEN
        SELECT active_release INTO NEW.managed_release FROM pack WHERE id = owner_id;
    END IF;
    RETURN NEW;
END $$;

DO $$
DECLARE component TEXT;
BEGIN
    FOREACH component IN ARRAY ARRAY[
        'runtime', 'permission_set', 'trigger', 'action', 'sensor', 'rule',
        'policy', 'work_queue', 'workflow_definition', 'dashboard', 'cache_namespace'
    ] LOOP
        EXECUTE format('CREATE TRIGGER set_component_managed_release BEFORE INSERT OR UPDATE ON %I
            FOR EACH ROW EXECUTE FUNCTION set_component_managed_release(%L)',
            component, CASE WHEN component = 'cache_namespace' THEN 'managing_pack' ELSE 'pack' END);
    END LOOP;
END $$;

-- New binaries opt into catalog writes only within the reconciliation transaction.
-- This is an application invariant, not authorization against a SQL credential holder.
CREATE FUNCTION guard_platform_component() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        IF OLD.catalog_revision IS NOT NULL THEN
            RAISE EXCEPTION 'platform-owned % cannot be deleted', TG_TABLE_NAME;
        END IF;
        RETURN OLD;
    END IF;
    IF NEW.catalog_revision IS NOT NULL OR
       (TG_OP = 'UPDATE' AND OLD.catalog_revision IS NOT NULL) THEN
        IF current_setting('attune.catalog_write', true) IS DISTINCT FROM 'on' THEN
            RAISE EXCEPTION 'platform-owned % may only be changed by catalog reconciliation', TG_TABLE_NAME;
        END IF;
        IF TG_OP = 'UPDATE' AND OLD.catalog_revision IS NOT NULL AND
           (NEW.catalog_revision IS NULL OR NEW.catalog_revision < OLD.catalog_revision OR
            (to_jsonb(NEW)->'ref') IS DISTINCT FROM (to_jsonb(OLD)->'ref')) THEN
            RAISE EXCEPTION 'platform ownership and revision cannot be downgraded';
        END IF;
    END IF;
    RETURN NEW;
END $$;

DO $$
DECLARE component TEXT;
BEGIN
    FOREACH component IN ARRAY ARRAY[
        'runtime', 'permission_set', 'trigger', 'action', 'sensor', 'rule',
        'policy', 'work_queue', 'workflow_definition', 'dashboard', 'cache_namespace',
        'intrinsic_handler'
    ] LOOP
        EXECUTE format('CREATE TRIGGER guard_platform_component BEFORE INSERT OR UPDATE OR DELETE ON %I
            FOR EACH ROW EXECUTE FUNCTION guard_platform_component()', component);
    END LOOP;
END $$;

CREATE FUNCTION guard_platform_runtime_version() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE parent_id BIGINT;
BEGIN
    parent_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.runtime ELSE NEW.runtime END;
    IF EXISTS (SELECT 1 FROM runtime WHERE id = parent_id AND catalog_revision IS NOT NULL)
       OR (TG_OP = 'UPDATE' AND EXISTS (
           SELECT 1 FROM runtime WHERE id = OLD.runtime AND catalog_revision IS NOT NULL
       )) THEN
        -- Host verification is operational state, not catalog metadata.
        IF TG_OP = 'UPDATE' AND
           (to_jsonb(NEW) - ARRAY['available', 'verified_at', 'updated']) =
           (to_jsonb(OLD) - ARRAY['available', 'verified_at', 'updated']) THEN
            RETURN NEW;
        END IF;
        IF current_setting('attune.catalog_write', true) IS DISTINCT FROM 'on' THEN
            RAISE EXCEPTION 'platform runtime versions may only be changed by catalog reconciliation';
        END IF;
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_platform_runtime_version BEFORE INSERT OR UPDATE OR DELETE ON runtime_version
    FOR EACH ROW EXECUTE FUNCTION guard_platform_runtime_version();
