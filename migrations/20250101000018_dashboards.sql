-- Migration: Dashboards foundation
-- Description: Adds dashboard metadata, scoped uniqueness/default-home semantics,
--              and immutable dashboard version history for optimistic concurrency.
-- Version: 20250101000018

SET search_path TO attune, public;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_type t
        JOIN pg_namespace n ON n.oid = t.typnamespace
        WHERE t.typname = 'dashboard_scope_type_enum'
          AND n.nspname = current_schema()
    ) THEN
        CREATE TYPE dashboard_scope_type_enum AS ENUM ('global', 'pack', 'identity', 'tenant');
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_type t
        JOIN pg_namespace n ON n.oid = t.typnamespace
        WHERE t.typname = 'dashboard_visibility_enum'
          AND n.nspname = current_schema()
    ) THEN
        CREATE TYPE dashboard_visibility_enum AS ENUM ('private', 'pack', 'public');
    END IF;
END $$;

CREATE TABLE dashboard (
    id BIGSERIAL PRIMARY KEY,
    ref TEXT NOT NULL,
    scope_type dashboard_scope_type_enum NOT NULL DEFAULT 'global',
    scope_ref TEXT NOT NULL DEFAULT 'global',
    pack BIGINT REFERENCES pack(id) ON DELETE CASCADE,
    owner_identity BIGINT REFERENCES identity(id) ON DELETE SET NULL,
    visibility dashboard_visibility_enum NOT NULL DEFAULT 'private',
    is_adhoc BOOLEAN NOT NULL DEFAULT false,
    label TEXT NOT NULL,
    description TEXT,
    enabled BOOLEAN NOT NULL DEFAULT true,
    is_default_home BOOLEAN NOT NULL DEFAULT false,
    revision INTEGER NOT NULL DEFAULT 1,
    spec_version INTEGER NOT NULL,
    spec JSONB NOT NULL,
    tags TEXT[] NOT NULL DEFAULT '{}',
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT dashboard_ref_lowercase CHECK (ref = LOWER(ref)),
    CONSTRAINT dashboard_ref_format CHECK (ref ~ '^[a-z0-9][a-z0-9_-]*(\.[a-z0-9][a-z0-9_-]*)+$'),
    CONSTRAINT dashboard_scope_ref_non_empty CHECK (length(trim(scope_ref)) > 0),
    CONSTRAINT dashboard_revision_positive CHECK (revision > 0),
    CONSTRAINT dashboard_spec_version_positive CHECK (spec_version > 0)
);

CREATE TABLE dashboard_version (
    id BIGSERIAL PRIMARY KEY,
    dashboard BIGINT NOT NULL REFERENCES dashboard(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    spec_version INTEGER NOT NULL,
    spec JSONB NOT NULL,
    created_by BIGINT REFERENCES identity(id) ON DELETE SET NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT dashboard_version_revision_positive CHECK (revision > 0),
    CONSTRAINT dashboard_version_spec_version_positive CHECK (spec_version > 0),
    CONSTRAINT uq_dashboard_version_dashboard_revision UNIQUE (dashboard, revision)
);

CREATE UNIQUE INDEX uq_dashboard_scope_ref
    ON dashboard(scope_type, scope_ref, ref);
CREATE UNIQUE INDEX uq_dashboard_default_home_scope
    ON dashboard(scope_type, scope_ref)
    WHERE is_default_home = TRUE;

CREATE INDEX idx_dashboard_pack ON dashboard(pack);
CREATE INDEX idx_dashboard_owner_identity ON dashboard(owner_identity);
CREATE INDEX idx_dashboard_scope_lookup ON dashboard(scope_type, scope_ref, enabled, ref);
CREATE INDEX idx_dashboard_visibility ON dashboard(visibility);
CREATE INDEX idx_dashboard_tags_gin ON dashboard USING GIN(tags);
CREATE INDEX idx_dashboard_spec_gin ON dashboard USING GIN(spec);
CREATE INDEX idx_dashboard_created ON dashboard(created DESC);

CREATE INDEX idx_dashboard_version_dashboard_created
    ON dashboard_version(dashboard, created DESC);

CREATE OR REPLACE FUNCTION enforce_dashboard_default_home()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.is_default_home AND (
        TG_OP = 'INSERT'
        OR OLD.is_default_home IS DISTINCT FROM TRUE
        OR OLD.scope_type IS DISTINCT FROM NEW.scope_type
        OR OLD.scope_ref IS DISTINCT FROM NEW.scope_ref
    ) THEN
        PERFORM pg_advisory_xact_lock(
            hashtextextended(NEW.scope_type::text || ':' || NEW.scope_ref, 0)
        );
        UPDATE dashboard
        SET is_default_home = FALSE,
            revision = revision + 1,
            updated = NOW()
        WHERE scope_type = NEW.scope_type
          AND scope_ref = NEW.scope_ref
          AND is_default_home = TRUE
          AND id <> NEW.id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER enforce_dashboard_default_home_trigger
    BEFORE INSERT OR UPDATE ON dashboard
    FOR EACH ROW
    EXECUTE FUNCTION enforce_dashboard_default_home();

CREATE TRIGGER update_dashboard_updated
    BEFORE UPDATE ON dashboard
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE dashboard IS 'Dashboard metadata and current declarative spec';
COMMENT ON COLUMN dashboard.scope_type IS 'Scope dimension for uniqueness/visibility resolution';
COMMENT ON COLUMN dashboard.scope_ref IS 'Scope instance identifier (e.g. global, pack ref, identity id, tenant id)';
COMMENT ON COLUMN dashboard.revision IS 'Optimistic concurrency revision; increments on each metadata/spec update';
COMMENT ON COLUMN dashboard.spec_version IS 'Dashboard spec schema version from declarative document';
COMMENT ON COLUMN dashboard.spec IS 'Full dashboard declarative spec JSONB';
COMMENT ON COLUMN dashboard.is_default_home IS 'Whether this dashboard is the default home dashboard for its scope';

COMMENT ON TABLE dashboard_version IS 'Immutable dashboard spec revisions for auditing and rollback';
COMMENT ON COLUMN dashboard_version.revision IS 'Snapshot of dashboard.revision at write time';
COMMENT ON COLUMN dashboard_version.created_by IS 'Identity that authored the revision when available';
