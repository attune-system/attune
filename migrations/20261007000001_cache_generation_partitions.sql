-- Fresh-install cache LIST partition lifecycle. All helpers are SECURITY INVOKER.
SET search_path TO attune, public;

-- Entry reclamation avoids row deletion, but subordinate metadata still needs
-- row work. Bound both sources independently, including empty upload chunks and
-- terminal iteration rows which can otherwise grow without a byte-quota cost.
ALTER TABLE cache_generation ADD CONSTRAINT cache_generation_chunk_metadata_bound
    CHECK (expected_chunk_count <= 10000);
ALTER TABLE cache_ingest_chunk ADD CONSTRAINT cache_ingest_chunk_metadata_bound
    CHECK (chunk_index < 10000);

ALTER TABLE cache_generation_entry_usage ADD COLUMN retained_iterations BIGINT NOT NULL DEFAULT 0
    CHECK (retained_iterations BETWEEN 0 AND 10000);

CREATE TABLE cache_entry_statistics_state (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    requested_revision BIGINT NOT NULL DEFAULT 0 CHECK (requested_revision >= 0),
    completed_revision BIGINT NOT NULL DEFAULT 0 CHECK (completed_revision >= 0),
    partitions_created BIGINT NOT NULL DEFAULT 0 CHECK (partitions_created >= 0),
    partitions_dropped BIGINT NOT NULL DEFAULT 0 CHECK (partitions_dropped >= 0),
    last_analyzed_at TIMESTAMPTZ,
    CHECK (completed_revision <= requested_revision)
);
INSERT INTO cache_entry_statistics_state(id) VALUES (TRUE);

CREATE FUNCTION request_cache_entry_statistics()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP IN ('INSERT', 'DELETE') OR NEW.state IS DISTINCT FROM OLD.state THEN
        UPDATE cache_entry_statistics_state
           SET requested_revision = requested_revision + 1 WHERE id = TRUE;
        IF NOT FOUND THEN RAISE EXCEPTION 'cache statistics state is missing'; END IF;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER request_cache_entry_statistics_trigger
    AFTER INSERT OR UPDATE OF state OR DELETE ON cache_generation
    FOR EACH ROW EXECUTE FUNCTION request_cache_entry_statistics();

CREATE FUNCTION bound_cache_iteration_metadata()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    LOCK TABLE ONLY cache_entry IN ACCESS SHARE MODE;
    PERFORM 1 FROM cache_generation WHERE id = NEW.generation FOR SHARE;
    -- An updated counter also protects REPEATABLE READ callers: a stale
    -- snapshot must fail serialization rather than admit beyond the cap.
    UPDATE cache_generation_entry_usage SET retained_iterations = retained_iterations + 1
     WHERE generation = NEW.generation AND retained_iterations < 10000;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'cache generation retained iteration metadata limit exceeded'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER bound_cache_iteration_metadata_trigger
    AFTER INSERT ON workflow_cache_iteration
    FOR EACH ROW EXECUTE FUNCTION bound_cache_iteration_metadata();

CREATE FUNCTION preserve_cache_iteration_generation()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.generation IS DISTINCT FROM OLD.generation OR NEW.namespace IS DISTINCT FROM OLD.namespace THEN
        RAISE EXCEPTION 'cache iteration generation is immutable';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER preserve_cache_iteration_generation_trigger
    BEFORE UPDATE OF generation, namespace ON workflow_cache_iteration
    FOR EACH ROW EXECUTE FUNCTION preserve_cache_iteration_generation();

CREATE FUNCTION release_cache_iteration_metadata()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    PERFORM u.generation FROM cache_generation_entry_usage u
      JOIN (SELECT DISTINCT generation FROM removed_cache_iterations) removed
        ON removed.generation = u.generation
     ORDER BY u.generation FOR UPDATE OF u;
    IF EXISTS (
        SELECT 1 FROM removed_cache_iterations removed
        JOIN cache_generation g ON g.id = removed.generation
        WHERE NOT EXISTS (SELECT 1 FROM cache_generation_entry_usage u WHERE u.generation = removed.generation)
    ) THEN
        RAISE EXCEPTION 'cache retained iteration metadata usage is missing';
    END IF;
    UPDATE cache_generation_entry_usage u
       SET retained_iterations = u.retained_iterations - removed.count
      FROM (SELECT generation, COUNT(*) AS count FROM removed_cache_iterations GROUP BY generation) removed
     WHERE u.generation = removed.generation;
    -- The nonnegative counter constraint rejects underflow. Grouped accounting
    -- avoids one SPI update per row during a high-cardinality terminal cascade.
    -- Usage may already be deleted by the same guarded reclamation transaction.
    RETURN NULL;
END;
$$;
CREATE TRIGGER release_cache_iteration_metadata_trigger
    AFTER DELETE ON workflow_cache_iteration
    REFERENCING OLD TABLE AS removed_cache_iterations
    FOR EACH STATEMENT EXECUTE FUNCTION release_cache_iteration_metadata();

CREATE FUNCTION cache_generation_partition_name(generation_id BIGINT)
RETURNS TEXT LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    IF generation_id IS NULL OR generation_id <= 0 THEN
        RAISE EXCEPTION 'cache generation ID must be positive';
    END IF;
    RETURN 'cache_entry_g_' || generation_id::TEXT;
END;
$$;

CREATE FUNCTION validate_cache_generation_partition(generation_id BIGINT)
RETURNS TEXT LANGUAGE plpgsql AS $$
DECLARE
    parent_id OID := 'cache_entry'::REGCLASS;
    parent_schema TEXT;
    child_name TEXT := cache_generation_partition_name(generation_id);
    child_id OID;
BEGIN
    SELECT n.nspname INTO STRICT parent_schema
      FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
     WHERE c.oid = parent_id AND c.relkind = 'p';
    SELECT c.oid INTO child_id
      FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
      JOIN pg_inherits i ON i.inhrelid = c.oid AND i.inhparent = parent_id
     WHERE n.nspname = parent_schema AND c.relname = child_name
       AND c.relkind = 'r' AND c.relispartition
       AND pg_get_expr(c.relpartbound, c.oid) = format('FOR VALUES IN (%L)', generation_id::TEXT);
    IF child_id IS NULL OR NOT EXISTS (
        SELECT 1 FROM cache_generation_entry_usage WHERE generation = generation_id
    ) THEN
        RAISE EXCEPTION 'cache generation partition or usage invariant is invalid';
    END IF;
    RETURN format('%I.%I', parent_schema, child_name);
END;
$$;

CREATE FUNCTION create_cache_generation_partition(generation_id BIGINT)
RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    parent_schema TEXT;
    parent_owner TEXT;
    child_name TEXT := cache_generation_partition_name(generation_id);
    child_relation TEXT;
BEGIN
    -- Caller holds admission and SHARE UPDATE EXCLUSIVE parent protection.
    SELECT n.nspname, pg_get_userbyid(c.relowner) INTO STRICT parent_schema, parent_owner
      FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
     WHERE c.oid = 'cache_entry'::REGCLASS AND c.relkind = 'p';
    child_relation := format('%I.%I', parent_schema, child_name);
    INSERT INTO cache_owner_physical_byte_usage(owner_type, owner, physical_bytes)
        SELECT n.owner_type, n.owner, 0 FROM cache_generation g
        JOIN cache_namespace n ON n.id = g.namespace WHERE g.id = generation_id
        ON CONFLICT (owner_type, owner) DO NOTHING;
    INSERT INTO cache_generation_entry_usage(generation) VALUES (generation_id);
    EXECUTE format('CREATE TABLE %s (LIKE cache_entry INCLUDING DEFAULTS INCLUDING CONSTRAINTS)', child_relation);
    EXECUTE format('ALTER TABLE %s ADD CHECK (generation = %s)', child_relation, generation_id);
    EXECUTE format('ALTER TABLE cache_entry ATTACH PARTITION %s FOR VALUES IN (%s)', child_relation, generation_id);
    -- Distinct API and supervisor logins may inherit the same owner role.
    -- Leaving the leaf owned by the API login would prevent supervisor DROP.
    EXECUTE format('ALTER TABLE %s OWNER TO %I', child_relation, parent_owner);
    PERFORM validate_cache_generation_partition(generation_id);
    UPDATE cache_entry_statistics_state SET partitions_created = partitions_created + 1 WHERE id = TRUE;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache statistics state is missing'; END IF;
END;
$$;

CREATE FUNCTION drop_cleanup_cache_generation(generation_id BIGINT, traversal_seconds BIGINT)
RETURNS TABLE(outcome TEXT, records_reclaimed BIGINT, bytes_reclaimed BIGINT)
LANGUAGE plpgsql AS $$
DECLARE
    namespace_id BIGINT;
    namespace_row cache_namespace%ROWTYPE;
    generation_row cache_generation%ROWTYPE;
    usage_row cache_generation_entry_usage%ROWTYPE;
    child_relation TEXT;
BEGIN
    IF traversal_seconds < 0 OR traversal_seconds IS NULL THEN
        RAISE EXCEPTION 'cache traversal window must be nonnegative';
    END IF;
    PERFORM pg_advisory_xact_lock(7821101, 0);
    LOCK TABLE ONLY cache_entry IN ACCESS EXCLUSIVE MODE;
    SELECT namespace INTO namespace_id FROM cache_generation WHERE id = generation_id;
    IF NOT FOUND THEN
        RETURN QUERY SELECT 'absent'::TEXT, 0::BIGINT, 0::BIGINT;
        RETURN;
    END IF;
    SELECT * INTO STRICT namespace_row FROM cache_namespace WHERE id = namespace_id FOR UPDATE;
    SELECT * INTO STRICT generation_row FROM cache_generation WHERE id = generation_id FOR UPDATE;
    IF namespace_row.active_generation IS NOT DISTINCT FROM generation_id
       OR NOT COALESCE(generation_row.state = 'failed' OR (
            generation_row.state = 'retired'
            AND generation_row.readable_until <= clock_timestamp()
            AND generation_row.retired <= clock_timestamp() - make_interval(secs => traversal_seconds::DOUBLE PRECISION)
       ), FALSE)
       OR EXISTS (
            SELECT 1 FROM workflow_cache_iteration i JOIN workflow_execution w ON w.id = i.workflow_execution
             WHERE i.generation = generation_id AND i.state = 'scanning'
               AND w.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
       ) THEN
        RETURN QUERY SELECT 'ineligible'::TEXT, 0::BIGINT, 0::BIGINT;
        RETURN;
    END IF;
    child_relation := validate_cache_generation_partition(generation_id);
    PERFORM 1 FROM cache_deployment_physical_byte_usage WHERE id = 1 FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache deployment usage is missing'; END IF;
    PERFORM 1 FROM cache_owner_physical_byte_usage
      WHERE owner_type = namespace_row.owner_type AND owner = namespace_row.owner FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache owner usage is missing'; END IF;
    SELECT * INTO STRICT usage_row FROM cache_generation_entry_usage WHERE generation = generation_id FOR UPDATE;
    EXECUTE format('DROP TABLE %s', child_relation);
    UPDATE cache_deployment_physical_byte_usage
       SET physical_bytes = physical_bytes - usage_row.physical_bytes
     WHERE id = 1 AND physical_bytes >= usage_row.physical_bytes;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache deployment usage underflow'; END IF;
    UPDATE cache_owner_physical_byte_usage
       SET physical_bytes = physical_bytes - usage_row.physical_bytes
     WHERE owner_type = namespace_row.owner_type AND owner = namespace_row.owner
       AND physical_bytes >= usage_row.physical_bytes;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache owner usage underflow'; END IF;
    DELETE FROM cache_generation_entry_usage WHERE generation = generation_id;
    DELETE FROM cache_ingest_chunk WHERE generation = generation_id;
    DELETE FROM cache_generation WHERE id = generation_id;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache generation disappeared during reclamation'; END IF;
    UPDATE cache_entry_statistics_state SET partitions_dropped = partitions_dropped + 1 WHERE id = TRUE;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache statistics state is missing'; END IF;
    RETURN QUERY SELECT 'dropped'::TEXT, usage_row.record_count, usage_row.physical_bytes;
END;
$$;

COMMENT ON TABLE cache_generation_entry_usage IS 'Exact admitted entry totals; charged until atomic partition reclamation commits';
