-- Database-local snapshot cloning and bounded staging-only delta ingestion.
-- Clone lineage is historical: reclaiming a base does not affect its copies.
ALTER TABLE cache_generation ADD COLUMN source_generation BIGINT
    CHECK (source_generation IS NULL OR source_generation > 0);

CREATE TABLE cache_generation_clone (
    generation BIGINT PRIMARY KEY REFERENCES cache_generation(id) ON DELETE CASCADE,
    record_count BIGINT NOT NULL CHECK (record_count >= 0),
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0)
);

-- For a cloned generation these are the signed net effects of a delta chunk.
-- Full-snapshot chunks continue to record their inserted records and bytes.
ALTER TABLE cache_ingest_chunk DROP CONSTRAINT cache_ingest_chunk_record_count_nonnegative;
ALTER TABLE cache_ingest_chunk DROP CONSTRAINT cache_ingest_chunk_size_bytes_nonnegative;

CREATE FUNCTION validate_cache_chunk_signed_totals()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.record_count < 0 OR NEW.size_bytes < 0) AND NOT EXISTS (
        SELECT 1 FROM cache_generation WHERE id = NEW.generation AND source_generation IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'negative cache chunk totals require a cloned generation'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER validate_cache_chunk_signed_totals_trigger
    BEFORE INSERT OR UPDATE ON cache_ingest_chunk
    FOR EACH ROW EXECUTE FUNCTION validate_cache_chunk_signed_totals();

CREATE FUNCTION preserve_cache_clone_lineage()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.source_generation IS DISTINCT FROM OLD.source_generation THEN
        RAISE EXCEPTION 'cache generation clone lineage is immutable';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER preserve_cache_clone_lineage_trigger
    BEFORE UPDATE OF source_generation ON cache_generation
    FOR EACH ROW EXECUTE FUNCTION preserve_cache_clone_lineage();

CREATE OR REPLACE FUNCTION cache_entry_staging_only()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    generation_id BIGINT;
    generation_state cache_generation_state_enum;
    source_id BIGINT;
    namespace_tombstoned TIMESTAMPTZ;
BEGIN
    -- Updates remain forbidden. Delta upserts replace records only in an
    -- unpublished clone, with DELETE and INSERT accounted independently.
    IF TG_OP = 'UPDATE' THEN
        RAISE EXCEPTION 'cache entries are immutable';
    END IF;
    generation_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.generation ELSE NEW.generation END;
    SELECT g.state, g.source_generation, n.tombstoned_at
      INTO generation_state, source_id, namespace_tombstoned
      FROM cache_generation g JOIN cache_namespace n ON n.id = g.namespace
     WHERE g.id = generation_id FOR SHARE OF g, n;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'cache generation does not exist';
    END IF;
    IF namespace_tombstoned IS NOT NULL THEN
        RAISE EXCEPTION 'cache namespace is tombstoned';
    END IF;
    IF generation_state <> 'staging' THEN
        RAISE EXCEPTION 'cache entries may only be written to staging generations';
    END IF;
    IF TG_OP = 'DELETE' THEN
        IF source_id IS NULL THEN
            RAISE EXCEPTION 'cache entries are immutable';
        END IF;
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION account_deleted_cache_entry_physical_bytes()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    deployment_delta BIGINT;
    owner_delta RECORD;
    generation_delta RECORD;
BEGIN
    SELECT COALESCE(SUM(size_bytes), 0)::BIGINT INTO deployment_delta
      FROM deleted_cache_entries;
    UPDATE cache_deployment_physical_byte_usage
       SET physical_bytes = physical_bytes - deployment_delta WHERE id = 1;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache deployment usage is missing'; END IF;
    FOR owner_delta IN
        SELECT n.owner_type, n.owner, SUM(e.size_bytes)::BIGINT AS bytes
          FROM deleted_cache_entries e
          JOIN cache_generation g ON g.id = e.generation
          JOIN cache_namespace n ON n.id = g.namespace
         GROUP BY n.owner_type, n.owner
         ORDER BY n.owner_type::TEXT COLLATE "C", n.owner COLLATE "C"
    LOOP
        UPDATE cache_owner_physical_byte_usage
           SET physical_bytes = physical_bytes - owner_delta.bytes
         WHERE owner_type = owner_delta.owner_type AND owner = owner_delta.owner;
        IF NOT FOUND THEN RAISE EXCEPTION 'cache owner usage is missing'; END IF;
    END LOOP;
    FOR generation_delta IN
        SELECT generation, COUNT(*)::BIGINT AS records, SUM(size_bytes)::BIGINT AS bytes
          FROM deleted_cache_entries GROUP BY generation ORDER BY generation
    LOOP
        UPDATE cache_generation_entry_usage
           SET record_count = record_count - generation_delta.records,
               physical_bytes = physical_bytes - generation_delta.bytes
         WHERE generation = generation_delta.generation;
        IF NOT FOUND THEN RAISE EXCEPTION 'cache generation entry usage is missing'; END IF;
    END LOOP;
    RETURN NULL;
END;
$$;
CREATE TRIGGER account_deleted_cache_entry_physical_bytes_trigger
    AFTER DELETE ON cache_entry REFERENCING OLD TABLE AS deleted_cache_entries
    FOR EACH STATEMENT EXECUTE FUNCTION account_deleted_cache_entry_physical_bytes();

COMMENT ON TABLE cache_generation_clone IS
    'Exact database-local clone baseline; seal validates baseline plus signed delta chunks';
