-- Prospective pages are source-prepared within the SAME atomic activation/refill
-- transaction. NULL iteration is an unpublished plan, never a fabricated row ID.
CREATE TABLE runtime_cache_iteration_epoch (
    iteration BIGINT PRIMARY KEY REFERENCES workflow_cache_iteration(id) ON DELETE CASCADE,
    creation_transaction TEXT NOT NULL
);
CREATE FUNCTION record_cache_iteration_epoch() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO runtime_cache_iteration_epoch(iteration,creation_transaction)
    VALUES(NEW.id,pg_current_xact_id()::TEXT);
    RETURN NEW;
END; $$;
CREATE TRIGGER record_cache_iteration_epoch AFTER INSERT ON workflow_cache_iteration
FOR EACH ROW EXECUTE FUNCTION record_cache_iteration_epoch();
CREATE TRIGGER guard_cache_iteration_epoch BEFORE UPDATE ON runtime_cache_iteration_epoch
FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE FUNCTION guard_cache_iteration_epoch_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM workflow_cache_iteration WHERE id=OLD.iteration) THEN
        RAISE EXCEPTION 'Cache iteration creation identity is immutable';
    END IF;
    RETURN OLD;
END; $$;
CREATE TRIGGER guard_cache_iteration_epoch_delete BEFORE DELETE ON runtime_cache_iteration_epoch
FOR EACH ROW EXECUTE FUNCTION guard_cache_iteration_epoch_delete();

CREATE TABLE runtime_native_cache_frontier (
    snapshot BIGINT PRIMARY KEY REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    predecessor BIGINT REFERENCES runtime_native_input_snapshot(id) ON DELETE RESTRICT,
    ordinal BIGINT NOT NULL CHECK(ordinal>=0),
    prepared_transaction TEXT NOT NULL DEFAULT pg_current_xact_id()::TEXT,
    bound_iteration BIGINT,
    bound_transaction TEXT,
    CHECK ((bound_iteration IS NULL) = (bound_transaction IS NULL)),
    CHECK (bound_transaction IS NULL OR bound_transaction=prepared_transaction),
    CHECK (predecessor IS NULL OR predecessor<snapshot)
);
CREATE INDEX runtime_native_cache_frontier_predecessor ON runtime_native_cache_frontier(predecessor);
CREATE UNIQUE INDEX runtime_native_cache_frontier_bound_slot
ON runtime_native_cache_frontier(bound_iteration,ordinal,bound_transaction)
WHERE bound_iteration IS NOT NULL;
CREATE FUNCTION guard_native_cache_frontier() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='INSERT' THEN
        IF NEW.prepared_transaction<>pg_current_xact_id()::TEXT OR NEW.bound_iteration IS NOT NULL THEN
            RAISE EXCEPTION 'Cache frontier must start unpublished in its preparation transaction';
        END IF;
    ELSIF TG_OP='DELETE' THEN
        IF EXISTS(SELECT 1 FROM runtime_native_input_snapshot WHERE id=OLD.snapshot) THEN
            RAISE EXCEPTION 'Native cache frontier evidence is immutable';
        END IF;
        RETURN OLD;
    ELSIF NEW.snapshot IS DISTINCT FROM OLD.snapshot OR NEW.predecessor IS DISTINCT FROM OLD.predecessor OR NEW.ordinal IS DISTINCT FROM OLD.ordinal
        OR NEW.prepared_transaction IS DISTINCT FROM OLD.prepared_transaction
        OR OLD.bound_iteration IS NOT NULL OR NEW.bound_iteration IS NULL
        OR NEW.bound_transaction IS DISTINCT FROM pg_current_xact_id()::TEXT
        OR NEW.prepared_transaction<>pg_current_xact_id()::TEXT THEN
        RAISE EXCEPTION 'Native cache frontier binding is immutable and transaction bound';
    END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER guard_native_cache_frontier BEFORE INSERT OR UPDATE OR DELETE ON runtime_native_cache_frontier
FOR EACH ROW EXECUTE FUNCTION guard_native_cache_frontier();

-- Reservation counterpart of A's typed storage_associations(), including a
-- maximum-width future iteration ID before initial target creation.
CREATE FUNCTION native_snapshot_storage_associations(body JSONB,renderer BIGINT)
RETURNS JSONB LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT CASE WHEN body#>>'{source,kind}'='wait' THEN
        jsonb_build_object('budget',renderer,'admissions',jsonb_build_array(body#>'{source,source_admission}'),
            'release',jsonb_build_array(body#>'{source,wait}',9223372036854775807,body#>'{source,prepared_transaction}'))
    ELSE jsonb_build_object('budget',renderer,'admissions','[]'::JSONB,'release',NULL)
        || CASE WHEN body#>'{source,frontier}' IS NULL THEN '{}'::JSONB ELSE
            jsonb_build_object('cache_frontier',jsonb_build_array(9223372036854775807,
                body#>'{source,frontier,predecessor}',body#>'{source,frontier,prepared_transaction}',
                body#>'{source,ordinal}',9223372036854775807,body#>'{source,frontier,prepared_transaction}'))
        END END
$$;
