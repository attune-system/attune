-- Live caches reuse one durable backing partition, without snapshot publication.
CREATE TYPE cache_mode_enum AS ENUM ('snapshot', 'live');
ALTER TABLE cache_namespace
    ADD COLUMN mode cache_mode_enum NOT NULL DEFAULT 'snapshot',
    ADD COLUMN live_revision BIGINT NOT NULL DEFAULT 0 CHECK (live_revision >= 0),
    ADD COLUMN live_updated_at TIMESTAMPTZ;

CREATE FUNCTION preserve_cache_namespace_mode() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.mode IS DISTINCT FROM OLD.mode THEN
        RAISE EXCEPTION 'cache namespace mode is immutable';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER preserve_cache_namespace_mode_trigger
    BEFORE UPDATE OF mode ON cache_namespace FOR EACH ROW
    EXECUTE FUNCTION preserve_cache_namespace_mode();

-- Bounded retry receipts contain metadata, never record values or external IDs.
CREATE TABLE cache_live_request (
    namespace BIGINT NOT NULL REFERENCES cache_namespace(id) ON DELETE CASCADE,
    request_id TEXT NOT NULL CHECK (octet_length(request_id) BETWEEN 1 AND 1024),
    request_checksum TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    record_count BIGINT NOT NULL CHECK (record_count >= 0),
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    source_revision TEXT,
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (namespace, request_id)
);
CREATE INDEX cache_live_request_expiry ON cache_live_request(namespace, revision);

CREATE FUNCTION validate_live_cache_generation() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE namespace_mode cache_mode_enum;
BEGIN
    SELECT mode INTO namespace_mode FROM cache_namespace WHERE id = NEW.namespace FOR UPDATE;
    IF namespace_mode = 'live' THEN
        IF NEW.state <> 'active' OR NEW.expected_chunk_count <> 0
           OR NEW.source_generation IS NOT NULL
           OR EXISTS (SELECT 1 FROM cache_generation WHERE namespace = NEW.namespace) THEN
            RAISE EXCEPTION 'live cache requires exactly one active backing generation';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER validate_live_cache_generation_trigger BEFORE INSERT ON cache_generation
    FOR EACH ROW EXECUTE FUNCTION validate_live_cache_generation();

CREATE OR REPLACE FUNCTION validate_cache_generation_transition() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE live_write BOOLEAN;
BEGIN
    SELECT mode = 'live' AND OLD.state = 'active' AND NEW.state = 'active'
      INTO live_write FROM cache_namespace WHERE id = OLD.namespace;
    IF NEW.namespace <> OLD.namespace
       OR NEW.client_refresh_id <> OLD.client_refresh_id
       OR NEW.expected_active_generation IS DISTINCT FROM OLD.expected_active_generation
       OR NEW.expected_chunk_count <> OLD.expected_chunk_count
       OR NEW.expected_count IS DISTINCT FROM OLD.expected_count
       OR NEW.expected_bytes IS DISTINCT FROM OLD.expected_bytes
       OR NEW.checksum_algorithm IS DISTINCT FROM OLD.checksum_algorithm
       OR NEW.checksum IS DISTINCT FROM OLD.checksum
       OR (NOT live_write AND NEW.source_revision IS DISTINCT FROM OLD.source_revision)
       OR NEW.created_by IS DISTINCT FROM OLD.created_by
       OR NEW.created_by_execution IS DISTINCT FROM OLD.created_by_execution THEN
        RAISE EXCEPTION 'cache generation identity and expected metadata are immutable';
    END IF;
    IF NEW.state <> OLD.state AND NOT (
        (OLD.state = 'staging' AND NEW.state IN ('ready', 'failed'))
        OR (OLD.state = 'ready' AND NEW.state IN ('active', 'failed'))
        OR (OLD.state = 'active' AND NEW.state = 'retired')
    ) THEN
        RAISE EXCEPTION 'invalid cache generation state transition';
    END IF;
    IF OLD.state <> 'staging' AND NOT live_write
       AND (NEW.record_count <> OLD.record_count OR NEW.size_bytes <> OLD.size_bytes) THEN
        RAISE EXCEPTION 'sealed cache generation counts are immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION cache_entry_staging_only() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    generation_id BIGINT;
    generation_state cache_generation_state_enum;
    source_id BIGINT;
    namespace_mode cache_mode_enum;
    namespace_tombstoned TIMESTAMPTZ;
BEGIN
    -- Both live replacements and staging deltas use accounted DELETE + INSERT.
    IF TG_OP = 'UPDATE' THEN RAISE EXCEPTION 'cache entry updates require replacement'; END IF;
    generation_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.generation ELSE NEW.generation END;
    SELECT g.state, g.source_generation, n.mode, n.tombstoned_at
      INTO generation_state, source_id, namespace_mode, namespace_tombstoned
      FROM cache_generation g JOIN cache_namespace n ON n.id = g.namespace
     WHERE g.id = generation_id FOR SHARE OF g, n;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache generation does not exist'; END IF;
    IF namespace_tombstoned IS NOT NULL THEN RAISE EXCEPTION 'cache namespace is tombstoned'; END IF;
    IF NOT (generation_state = 'staging' OR (namespace_mode = 'live' AND generation_state = 'active')) THEN
        RAISE EXCEPTION 'published cache snapshots are immutable';
    END IF;
    IF TG_OP = 'DELETE' THEN
        IF namespace_mode <> 'live' AND source_id IS NULL THEN
            RAISE EXCEPTION 'cache entries are immutable';
        END IF;
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION reject_live_cache_iteration() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM cache_namespace WHERE id = NEW.namespace AND mode = 'live') THEN
        RAISE EXCEPTION 'live caches cannot be iterated or pinned by workflows';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER reject_live_cache_iteration_trigger BEFORE INSERT ON workflow_cache_iteration
    FOR EACH ROW EXECUTE FUNCTION reject_live_cache_iteration();
