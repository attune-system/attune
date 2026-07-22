-- Migration: Owner-scoped cache generations
-- Description: Creates immutable cache namespace, generation, entry, and ingest chunk storage.
-- Version: 20250101000021

SET search_path TO attune, public;

DO $$ BEGIN
    CREATE TYPE cache_generation_state_enum AS ENUM (
        'staging',
        'ready',
        'active',
        'retired',
        'failed'
    );
EXCEPTION
    WHEN duplicate_object THEN NULL;
END $$;

ALTER TABLE runtime_retention_config
    ADD COLUMN IF NOT EXISTS cache_retention JSONB NOT NULL DEFAULT '{}'::JSONB;

CREATE TABLE cache_namespace (
    id BIGSERIAL PRIMARY KEY,
    owner_type owner_type_enum NOT NULL,
    owner TEXT NOT NULL DEFAULT '',
    owner_identity BIGINT REFERENCES identity(id) ON DELETE RESTRICT,
    owner_pack BIGINT REFERENCES pack(id) ON DELETE SET NULL,
    owner_pack_ref TEXT,
    owner_action BIGINT REFERENCES action(id) ON DELETE SET NULL,
    owner_action_ref TEXT,
    owner_sensor BIGINT REFERENCES sensor(id) ON DELETE SET NULL,
    owner_sensor_ref TEXT,
    definition_ref TEXT,
    managing_pack BIGINT REFERENCES pack(id) ON DELETE SET NULL,
    managing_pack_ref TEXT,
    namespace TEXT NOT NULL,
    active_generation BIGINT,
    freshness_target_seconds BIGINT NOT NULL DEFAULT 3600,
    max_records_per_generation BIGINT NOT NULL DEFAULT 200000,
    max_generation_bytes BIGINT NOT NULL DEFAULT 536870912,
    max_retained_bytes BIGINT NOT NULL DEFAULT 2147483648,
    max_retained_generations INTEGER NOT NULL DEFAULT 5,
    max_staging_generations INTEGER NOT NULL DEFAULT 2,
    tombstoned_at TIMESTAMPTZ,
    tombstone_reason TEXT,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT cache_namespace_namespace_format
        CHECK (namespace ~ '^[a-z0-9][a-z0-9._-]{0,127}$'),
    CONSTRAINT cache_namespace_namespace_lowercase
        CHECK (namespace = LOWER(namespace)),
    CONSTRAINT cache_namespace_owner_length
        CHECK (octet_length(owner) <= 64),
    CONSTRAINT cache_namespace_owner_pack_ref_length
        CHECK (owner_pack_ref IS NULL OR octet_length(owner_pack_ref) <= 1024),
    CONSTRAINT cache_namespace_owner_action_ref_length
        CHECK (owner_action_ref IS NULL OR octet_length(owner_action_ref) <= 1024),
    CONSTRAINT cache_namespace_owner_sensor_ref_length
        CHECK (owner_sensor_ref IS NULL OR octet_length(owner_sensor_ref) <= 1024),
    CONSTRAINT cache_namespace_definition_ref_length
        CHECK (definition_ref IS NULL OR octet_length(definition_ref) <= 1024),
    CONSTRAINT cache_namespace_managing_pack_ref_length
        CHECK (managing_pack_ref IS NULL OR octet_length(managing_pack_ref) <= 1024),
    CONSTRAINT cache_namespace_tombstone_reason_length
        CHECK (tombstone_reason IS NULL OR octet_length(tombstone_reason) <= 4096),
    CONSTRAINT cache_namespace_freshness_target_nonnegative
        CHECK (freshness_target_seconds >= 0),
    CONSTRAINT cache_namespace_record_limit_positive
        CHECK (max_records_per_generation >= 0),
    CONSTRAINT cache_namespace_generation_bytes_positive
        CHECK (max_generation_bytes >= 0),
    CONSTRAINT cache_namespace_retained_bytes_positive
        CHECK (max_retained_bytes >= 0),
    CONSTRAINT cache_namespace_retained_generations_positive
        CHECK (max_retained_generations >= 1),
    CONSTRAINT cache_namespace_staging_generations_positive
        CHECK (max_staging_generations >= 1),
    CONSTRAINT cache_namespace_management_fields
        CHECK (
            (definition_ref IS NULL AND managing_pack IS NULL AND managing_pack_ref IS NULL)
            OR (definition_ref IS NOT NULL AND managing_pack_ref IS NOT NULL)
        ),
    CONSTRAINT cache_namespace_id_namespace_unique UNIQUE (id, namespace)
);

CREATE UNIQUE INDEX cache_namespace_owner_namespace_unique
    ON cache_namespace (owner_type, owner, namespace)
    WHERE tombstoned_at IS NULL;
CREATE UNIQUE INDEX cache_namespace_live_definition_unique
    ON cache_namespace (managing_pack_ref, definition_ref)
    WHERE tombstoned_at IS NULL AND definition_ref IS NOT NULL;
CREATE INDEX cache_namespace_active_generation_idx
    ON cache_namespace (active_generation)
    WHERE active_generation IS NOT NULL;
CREATE INDEX cache_namespace_tombstoned_idx
    ON cache_namespace (tombstoned_at)
    WHERE tombstoned_at IS NOT NULL;
CREATE INDEX cache_namespace_live_id_idx
    ON cache_namespace (id)
    WHERE tombstoned_at IS NULL;

CREATE OR REPLACE FUNCTION validate_cache_namespace_owner()
RETURNS TRIGGER AS $$
DECLARE
    owner_count INTEGER := 0;
BEGIN
    IF NEW.owner_identity IS NOT NULL THEN owner_count := owner_count + 1; END IF;
    IF NEW.owner_pack IS NOT NULL THEN owner_count := owner_count + 1; END IF;
    IF NEW.owner_action IS NOT NULL THEN owner_count := owner_count + 1; END IF;
    IF NEW.owner_sensor IS NOT NULL THEN owner_count := owner_count + 1; END IF;

    IF NEW.definition_ref IS NULL THEN
        IF NEW.managing_pack IS NOT NULL OR NEW.managing_pack_ref IS NOT NULL THEN
            RAISE EXCEPTION 'API-created cache namespaces cannot have pack management fields';
        END IF;
    ELSE
        IF NEW.managing_pack_ref IS NULL THEN
            RAISE EXCEPTION 'pack-managed cache namespaces require managing_pack_ref';
        END IF;
        IF NEW.tombstoned_at IS NULL AND NEW.managing_pack IS NULL THEN
            RAISE EXCEPTION 'live pack-managed cache namespaces require managing_pack';
        END IF;
    END IF;

    IF NEW.owner_type = 'system' THEN
        IF owner_count <> 0
           OR NEW.owner_pack_ref IS NOT NULL
           OR NEW.owner_action_ref IS NOT NULL
           OR NEW.owner_sensor_ref IS NOT NULL THEN
            RAISE EXCEPTION 'system cache namespaces cannot have owner fields';
        END IF;
        NEW.owner := 'system';
    ELSIF NEW.owner_type = 'identity' THEN
        IF owner_count <> 1
           OR NEW.owner_identity IS NULL
           OR NEW.owner_pack_ref IS NOT NULL
           OR NEW.owner_action_ref IS NOT NULL
           OR NEW.owner_sensor_ref IS NOT NULL THEN
            RAISE EXCEPTION 'owner_identity must be the only owner field for identity cache namespace';
        END IF;
        NEW.owner := NEW.owner_identity::TEXT;
    ELSIF NEW.owner_type = 'pack' THEN
        IF NEW.owner_identity IS NOT NULL
           OR NEW.owner_action IS NOT NULL
           OR NEW.owner_sensor IS NOT NULL
           OR NEW.owner_action_ref IS NOT NULL
           OR NEW.owner_sensor_ref IS NOT NULL THEN
            RAISE EXCEPTION 'owner_pack must be the only canonical owner ID for pack cache namespace';
        END IF;
        IF NEW.owner_pack IS NULL THEN
            IF NEW.tombstoned_at IS NULL OR NEW.owner = '' THEN
                RAISE EXCEPTION 'live pack cache namespaces require owner_pack';
            END IF;
        ELSE
            NEW.owner := NEW.owner_pack::TEXT;
        END IF;
    ELSIF NEW.owner_type = 'action' THEN
        IF NEW.owner_identity IS NOT NULL
           OR NEW.owner_pack IS NOT NULL
           OR NEW.owner_sensor IS NOT NULL
           OR NEW.owner_pack_ref IS NOT NULL
           OR NEW.owner_sensor_ref IS NOT NULL THEN
            RAISE EXCEPTION 'owner_action must be the only canonical owner ID for action cache namespace';
        END IF;
        IF NEW.owner_action IS NULL THEN
            IF NEW.tombstoned_at IS NULL OR NEW.owner = '' THEN
                RAISE EXCEPTION 'live action cache namespaces require owner_action';
            END IF;
        ELSE
            NEW.owner := NEW.owner_action::TEXT;
        END IF;
    ELSIF NEW.owner_type = 'sensor' THEN
        IF NEW.owner_identity IS NOT NULL
           OR NEW.owner_pack IS NOT NULL
           OR NEW.owner_action IS NOT NULL
           OR NEW.owner_pack_ref IS NOT NULL
           OR NEW.owner_action_ref IS NOT NULL THEN
            RAISE EXCEPTION 'owner_sensor must be the only canonical owner ID for sensor cache namespace';
        END IF;
        IF NEW.owner_sensor IS NULL THEN
            IF NEW.tombstoned_at IS NULL OR NEW.owner = '' THEN
                RAISE EXCEPTION 'live sensor cache namespaces require owner_sensor';
            END IF;
        ELSE
            NEW.owner := NEW.owner_sensor::TEXT;
        END IF;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER validate_cache_namespace_owner_trigger
    BEFORE INSERT OR UPDATE ON cache_namespace
    FOR EACH ROW
    EXECUTE FUNCTION validate_cache_namespace_owner();

CREATE TRIGGER update_cache_namespace_updated
    BEFORE UPDATE ON cache_namespace
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

CREATE TABLE cache_generation (
    id BIGSERIAL PRIMARY KEY,
    namespace BIGINT NOT NULL REFERENCES cache_namespace(id) ON DELETE RESTRICT,
    state cache_generation_state_enum NOT NULL DEFAULT 'staging',
    client_refresh_id TEXT NOT NULL,
    expected_active_generation BIGINT,
    expected_chunk_count INTEGER NOT NULL,
    expected_count BIGINT,
    expected_bytes BIGINT,
    record_count BIGINT NOT NULL DEFAULT 0,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    checksum_algorithm TEXT,
    checksum TEXT,
    source_revision TEXT,
    created_by BIGINT REFERENCES identity(id) ON DELETE SET NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    sealed TIMESTAMPTZ,
    activated TIMESTAMPTZ,
    retired TIMESTAMPTZ,
    readable_until TIMESTAMPTZ,
    failed TIMESTAMPTZ,
    failure_reason TEXT,

    CONSTRAINT cache_generation_id_namespace_unique UNIQUE (id, namespace),
    CONSTRAINT cache_generation_client_refresh_unique UNIQUE (namespace, client_refresh_id),
    CONSTRAINT cache_generation_client_refresh_nonempty CHECK (btrim(client_refresh_id) <> ''),
    CONSTRAINT cache_generation_client_refresh_length
        CHECK (octet_length(client_refresh_id) <= 1024),
    CONSTRAINT cache_generation_checksum_algorithm_length
        CHECK (checksum_algorithm IS NULL OR octet_length(checksum_algorithm) <= 64),
    CONSTRAINT cache_generation_checksum_length
        CHECK (checksum IS NULL OR octet_length(checksum) <= 1024),
    CONSTRAINT cache_generation_source_revision_length
        CHECK (source_revision IS NULL OR octet_length(source_revision) <= 1024),
    CONSTRAINT cache_generation_failure_reason_length
        CHECK (failure_reason IS NULL OR octet_length(failure_reason) <= 4096),
    CONSTRAINT cache_generation_expected_chunk_count_nonnegative CHECK (expected_chunk_count >= 0),
    CONSTRAINT cache_generation_expected_count_nonnegative
        CHECK (expected_count IS NULL OR expected_count >= 0),
    CONSTRAINT cache_generation_expected_bytes_nonnegative
        CHECK (expected_bytes IS NULL OR expected_bytes >= 0),
    CONSTRAINT cache_generation_record_count_nonnegative CHECK (record_count >= 0),
    CONSTRAINT cache_generation_size_bytes_nonnegative CHECK (size_bytes >= 0),
    CONSTRAINT cache_generation_checksum_pair
        CHECK ((checksum_algorithm IS NULL) = (checksum IS NULL))
);

ALTER TABLE cache_namespace
    ADD CONSTRAINT cache_namespace_active_generation_same_namespace_fkey
    FOREIGN KEY (active_generation, id)
    REFERENCES cache_generation(id, namespace)
    ON DELETE RESTRICT;

CREATE OR REPLACE FUNCTION validate_cache_namespace_active_generation()
RETURNS TRIGGER AS $$
DECLARE
    generation_state cache_generation_state_enum;
BEGIN
    IF NEW.active_generation IS NULL THEN
        RETURN NEW;
    END IF;

    SELECT state INTO generation_state
      FROM cache_generation
     WHERE id = NEW.active_generation AND namespace = NEW.id
     FOR SHARE;
    IF NOT FOUND OR generation_state <> 'active' THEN
        RAISE EXCEPTION 'cache namespace active_generation must reference an active generation in the same namespace';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER validate_cache_namespace_active_generation_trigger
    BEFORE INSERT OR UPDATE ON cache_namespace
    FOR EACH ROW
    EXECUTE FUNCTION validate_cache_namespace_active_generation();

CREATE UNIQUE INDEX cache_generation_one_active_per_namespace
    ON cache_generation (namespace)
    WHERE state = 'active';
CREATE INDEX cache_generation_namespace_state_created_idx
    ON cache_generation (namespace, state, created);
CREATE INDEX cache_generation_namespace_created_id_idx
    ON cache_generation (namespace, created DESC, id DESC);
CREATE INDEX cache_generation_state_readable_until_idx
    ON cache_generation (state, readable_until)
    WHERE state IN ('retired', 'failed');

CREATE OR REPLACE FUNCTION validate_cache_generation_transition()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.namespace <> OLD.namespace
       OR NEW.client_refresh_id <> OLD.client_refresh_id
       OR NEW.expected_active_generation IS DISTINCT FROM OLD.expected_active_generation
       OR NEW.expected_chunk_count <> OLD.expected_chunk_count
       OR NEW.expected_count IS DISTINCT FROM OLD.expected_count
       OR NEW.expected_bytes IS DISTINCT FROM OLD.expected_bytes
       OR NEW.checksum_algorithm IS DISTINCT FROM OLD.checksum_algorithm
       OR NEW.checksum IS DISTINCT FROM OLD.checksum
       OR NEW.source_revision IS DISTINCT FROM OLD.source_revision
       OR NEW.created_by IS DISTINCT FROM OLD.created_by THEN
        RAISE EXCEPTION 'cache generation identity and expected metadata are immutable';
    END IF;

    IF NEW.state <> OLD.state THEN
        IF NOT (
            (OLD.state = 'staging' AND NEW.state IN ('ready', 'failed'))
            OR (OLD.state = 'ready' AND NEW.state IN ('active', 'failed'))
            OR (OLD.state = 'active' AND NEW.state = 'retired')
        ) THEN
            RAISE EXCEPTION 'invalid cache generation state transition from % to %',
                OLD.state, NEW.state;
        END IF;
    END IF;

    IF OLD.state <> 'staging'
       AND (NEW.record_count <> OLD.record_count OR NEW.size_bytes <> OLD.size_bytes) THEN
        RAISE EXCEPTION 'sealed cache generation counts are immutable';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER validate_cache_generation_transition_trigger
    BEFORE UPDATE ON cache_generation
    FOR EACH ROW
    EXECUTE FUNCTION validate_cache_generation_transition();

CREATE TABLE cache_entry (
    id BIGSERIAL PRIMARY KEY,
    generation BIGINT NOT NULL REFERENCES cache_generation(id) ON DELETE RESTRICT,
    external_id TEXT COLLATE "C" NOT NULL,
    value JSONB NOT NULL,
    source_updated_at TIMESTAMPTZ,
    source_checksum TEXT,
    size_bytes BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT cache_entry_external_id_nonempty CHECK (external_id <> ''),
    CONSTRAINT cache_entry_external_id_length CHECK (octet_length(external_id) <= 1024),
    CONSTRAINT cache_entry_value_size
        CHECK (pg_column_size(value) <= 1048576 AND octet_length(value::TEXT) <= 1048576),
    CONSTRAINT cache_entry_source_checksum_length
        CHECK (source_checksum IS NULL OR octet_length(source_checksum) <= 1024),
    CONSTRAINT cache_entry_size_bytes_nonnegative CHECK (size_bytes >= 0)
);

CREATE UNIQUE INDEX cache_entry_generation_external_id_bytewise_unique
    ON cache_entry (generation, external_id COLLATE "C");
CREATE INDEX cache_entry_generation_id_idx ON cache_entry (generation, id);

CREATE OR REPLACE FUNCTION account_cache_entry_size()
RETURNS TRIGGER AS $$
BEGIN
    NEW.size_bytes :=
        pg_column_size(NEW.value)::BIGINT
        + octet_length(NEW.external_id)::BIGINT
        + COALESCE(octet_length(NEW.source_checksum), 0)::BIGINT;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER account_cache_entry_size_trigger
    BEFORE INSERT ON cache_entry
    FOR EACH ROW
    EXECUTE FUNCTION account_cache_entry_size();

CREATE OR REPLACE FUNCTION cache_entry_staging_only()
RETURNS TRIGGER AS $$
DECLARE
    generation_state cache_generation_state_enum;
    generation_readable_until TIMESTAMPTZ;
    namespace_tombstoned TIMESTAMPTZ;
BEGIN
    IF TG_OP = 'DELETE' THEN
        SELECT state, readable_until
          INTO generation_state, generation_readable_until
          FROM cache_generation
         WHERE id = OLD.generation
         FOR SHARE;
        IF generation_state <> 'failed'
           AND NOT (generation_state = 'retired'
                    AND generation_readable_until IS NOT NULL
                    AND generation_readable_until <= NOW()) THEN
            RAISE EXCEPTION 'cache entries may only be deleted from failed or expired retired generations';
        END IF;
        RETURN OLD;
    END IF;

    IF TG_OP = 'UPDATE' THEN
        RAISE EXCEPTION 'cache entries are immutable';
    END IF;

    SELECT g.state, n.tombstoned_at
      INTO generation_state, namespace_tombstoned
      FROM cache_generation g
      JOIN cache_namespace n ON n.id = g.namespace
     WHERE g.id = NEW.generation
     FOR SHARE OF g, n;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'cache generation % does not exist', NEW.generation;
    END IF;
    IF namespace_tombstoned IS NOT NULL THEN
        RAISE EXCEPTION 'cache namespace is tombstoned';
    END IF;
    IF generation_state <> 'staging' THEN
        RAISE EXCEPTION 'cache entries may only be inserted into staging generations';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER cache_entry_staging_only_trigger
    BEFORE INSERT OR UPDATE OR DELETE ON cache_entry
    FOR EACH ROW
    EXECUTE FUNCTION cache_entry_staging_only();

CREATE TABLE cache_ingest_chunk (
    id BIGSERIAL PRIMARY KEY,
    generation BIGINT NOT NULL REFERENCES cache_generation(id) ON DELETE RESTRICT,
    chunk_index INTEGER NOT NULL,
    request_checksum TEXT NOT NULL,
    record_count BIGINT NOT NULL,
    size_bytes BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT cache_ingest_chunk_generation_index_unique UNIQUE (generation, chunk_index),
    CONSTRAINT cache_ingest_chunk_index_nonnegative CHECK (chunk_index >= 0),
    CONSTRAINT cache_ingest_chunk_checksum_nonempty CHECK (btrim(request_checksum) <> ''),
    CONSTRAINT cache_ingest_chunk_checksum_length
        CHECK (octet_length(request_checksum) <= 1024),
    CONSTRAINT cache_ingest_chunk_record_count_nonnegative CHECK (record_count >= 0),
    CONSTRAINT cache_ingest_chunk_size_bytes_nonnegative CHECK (size_bytes >= 0)
);

CREATE INDEX cache_ingest_chunk_generation_index_idx
    ON cache_ingest_chunk (generation, chunk_index);

COMMENT ON TABLE cache_namespace IS
    'Owner-scoped cache datasets with canonical owner IDs and publication policy';
COMMENT ON TABLE cache_generation IS
    'Immutable copy-on-write cache snapshots; only the namespace active pointer is readable by default';
COMMENT ON TABLE cache_entry IS
    'Immutable cache records ordered by bytewise external identifier within a generation';
COMMENT ON TABLE cache_ingest_chunk IS
    'Idempotent accepted ingest request metadata for resumable cache generation uploads';
