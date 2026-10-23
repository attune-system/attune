-- Unit-only DELETE transition-accounting recipe, authored representative text.
-- Not a historical revision, executable schema, or benchmark provenance.
CREATE OR REPLACE FUNCTION account_deleted_cache_entry_physical_bytes()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    owner_delta RECORD;
BEGIN
    UPDATE cache_deployment_physical_byte_usage
       SET physical_bytes = physical_bytes - (
           SELECT COALESCE(SUM(size_bytes), 0) FROM deleted_cache_entries
       )
     WHERE id = TRUE;
    FOR owner_delta IN
        SELECT n.owner_type, n.owner, SUM(e.size_bytes)::BIGINT AS physical_bytes
          FROM deleted_cache_entries e
          JOIN cache_generation g ON g.id = e.generation
          JOIN cache_namespace n ON n.id = g.namespace
         GROUP BY n.owner_type, n.owner
         ORDER BY n.owner_type, n.owner
    LOOP
        UPDATE cache_owner_physical_byte_usage
           SET physical_bytes = physical_bytes - owner_delta.physical_bytes
         WHERE owner_type = owner_delta.owner_type AND owner = owner_delta.owner;
    END LOOP;
    RETURN NULL;
END; $$;

CREATE TRIGGER account_inserted_cache_entry_physical_bytes_trigger
-- Extraction boundary only; not part of the copied DELETE function.
