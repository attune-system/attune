-- Migration: Persisted cache physical-byte accounting
-- Description: Backfills and transactionally maintains deployment and canonical-owner usage.
-- Version: 20250101000024

SET search_path TO attune, public;

CREATE TABLE cache_deployment_physical_byte_usage (
    id SMALLINT PRIMARY KEY DEFAULT 1,
    physical_bytes BIGINT NOT NULL DEFAULT 0,

    CONSTRAINT cache_deployment_physical_byte_usage_singleton CHECK (id = 1),
    CONSTRAINT cache_deployment_physical_byte_usage_nonnegative CHECK (physical_bytes >= 0)
);

CREATE TABLE cache_owner_physical_byte_usage (
    owner_type owner_type_enum NOT NULL,
    owner TEXT NOT NULL,
    physical_bytes BIGINT NOT NULL DEFAULT 0,

    PRIMARY KEY (owner_type, owner),
    CONSTRAINT cache_owner_physical_byte_usage_nonnegative CHECK (physical_bytes >= 0)
);

-- Prevent entry writes between the exact backfill snapshot and trigger creation.
LOCK TABLE cache_entry IN SHARE ROW EXCLUSIVE MODE;

INSERT INTO cache_deployment_physical_byte_usage (id, physical_bytes)
SELECT 1, COALESCE(SUM(size_bytes), 0)::BIGINT
  FROM cache_entry;

INSERT INTO cache_owner_physical_byte_usage (owner_type, owner, physical_bytes)
SELECT n.owner_type, n.owner, SUM(e.size_bytes)::BIGINT
  FROM cache_entry e
  JOIN cache_generation g ON g.id = e.generation
  JOIN cache_namespace n ON n.id = g.namespace
 GROUP BY n.owner_type, n.owner;

CREATE OR REPLACE FUNCTION account_inserted_cache_entry_physical_bytes()
RETURNS TRIGGER AS $$
DECLARE
    deployment_delta BIGINT;
    owner_delta RECORD;
BEGIN
    SELECT COALESCE(SUM(size_bytes), 0)::BIGINT
      INTO deployment_delta
      FROM inserted_cache_entries;

    UPDATE cache_deployment_physical_byte_usage
       SET physical_bytes = physical_bytes + deployment_delta
     WHERE id = 1;

    -- All statements lock deployment first, then canonical owners bytewise.
    FOR owner_delta IN
        SELECT n.owner_type, n.owner, SUM(e.size_bytes)::BIGINT AS physical_bytes
          FROM inserted_cache_entries e
          JOIN cache_generation g ON g.id = e.generation
          JOIN cache_namespace n ON n.id = g.namespace
         GROUP BY n.owner_type, n.owner
         ORDER BY n.owner_type::TEXT COLLATE "C", n.owner COLLATE "C"
    LOOP
        INSERT INTO cache_owner_physical_byte_usage (owner_type, owner, physical_bytes)
        VALUES (owner_delta.owner_type, owner_delta.owner, 0)
        ON CONFLICT (owner_type, owner) DO NOTHING;

        PERFORM 1
          FROM cache_owner_physical_byte_usage
         WHERE owner_type = owner_delta.owner_type AND owner = owner_delta.owner
         FOR UPDATE;

        UPDATE cache_owner_physical_byte_usage
           SET physical_bytes = physical_bytes + owner_delta.physical_bytes
         WHERE owner_type = owner_delta.owner_type AND owner = owner_delta.owner;
    END LOOP;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION account_deleted_cache_entry_physical_bytes()
RETURNS TRIGGER AS $$
DECLARE
    deployment_delta BIGINT;
    owner_delta RECORD;
BEGIN
    SELECT COALESCE(SUM(size_bytes), 0)::BIGINT
      INTO deployment_delta
      FROM deleted_cache_entries;

    UPDATE cache_deployment_physical_byte_usage
       SET physical_bytes = physical_bytes - deployment_delta
     WHERE id = 1;

    -- Retain zero rows so future admissions can read without creating counters.
    FOR owner_delta IN
        SELECT n.owner_type, n.owner, SUM(e.size_bytes)::BIGINT AS physical_bytes
          FROM deleted_cache_entries e
          JOIN cache_generation g ON g.id = e.generation
          JOIN cache_namespace n ON n.id = g.namespace
         GROUP BY n.owner_type, n.owner
         ORDER BY n.owner_type::TEXT COLLATE "C", n.owner COLLATE "C"
    LOOP
        PERFORM 1
          FROM cache_owner_physical_byte_usage
         WHERE owner_type = owner_delta.owner_type AND owner = owner_delta.owner
         FOR UPDATE;

        UPDATE cache_owner_physical_byte_usage
           SET physical_bytes = physical_bytes - owner_delta.physical_bytes
         WHERE owner_type = owner_delta.owner_type AND owner = owner_delta.owner;
    END LOOP;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER account_inserted_cache_entry_physical_bytes_trigger
    AFTER INSERT ON cache_entry
    REFERENCING NEW TABLE AS inserted_cache_entries
    FOR EACH STATEMENT
    EXECUTE FUNCTION account_inserted_cache_entry_physical_bytes();

CREATE TRIGGER account_deleted_cache_entry_physical_bytes_trigger
    AFTER DELETE ON cache_entry
    REFERENCING OLD TABLE AS deleted_cache_entries
    FOR EACH STATEMENT
    EXECUTE FUNCTION account_deleted_cache_entry_physical_bytes();

COMMENT ON TABLE cache_deployment_physical_byte_usage IS
    'Singleton physical cache-entry byte total used by deployment admission';
COMMENT ON TABLE cache_owner_physical_byte_usage IS
    'Physical cache-entry byte totals keyed by canonical namespace owner';
