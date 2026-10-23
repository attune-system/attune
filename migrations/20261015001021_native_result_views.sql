-- Result projections are actual terminal-child observations, not waits or
-- fabricated authored output declarations. Raw execution IDs remain retainable.
CREATE TABLE runtime_native_result_view_release (
    snapshot BIGINT PRIMARY KEY REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    source_admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id),
    source_execution BIGINT NOT NULL CHECK(source_execution>0),
    release_transaction TEXT NOT NULL
);
CREATE FUNCTION protect_native_result_view_release() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' THEN RAISE EXCEPTION 'Native result view release is immutable'; END IF;
    IF TG_OP='DELETE' THEN
        IF EXISTS(SELECT 1 FROM runtime_native_input_snapshot WHERE id=OLD.snapshot) THEN
            RAISE EXCEPTION 'Native result view release belongs to its retained snapshot';
        END IF;
        RETURN OLD;
    END IF;
    IF NEW.release_transaction<>pg_current_xact_id()::TEXT OR NOT EXISTS(
        SELECT 1 FROM runtime_native_input_snapshot snapshot
        JOIN runtime_native_input_snapshot_admission admission ON admission.snapshot=snapshot.id
        WHERE snapshot.id=NEW.snapshot AND admission.admission=NEW.source_admission
          AND snapshot.body#>>'{source,kind}'='result_view'
          AND snapshot.body#>>'{source,transaction}'=NEW.release_transaction
          AND (snapshot.body#>>'{source,execution}')::BIGINT=NEW.source_execution
          AND (snapshot.body#>>'{source,admission}')::BIGINT=NEW.source_admission
    ) THEN RAISE EXCEPTION 'Native result release must match its actual prepared observation'; END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER protect_native_result_view_release BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_result_view_release FOR EACH ROW EXECUTE FUNCTION protect_native_result_view_release();

-- Extend the existing reservation function only through this forward migration.
CREATE OR REPLACE FUNCTION native_snapshot_storage_associations(body JSONB,renderer BIGINT)
RETURNS JSONB LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT CASE WHEN body#>>'{source,kind}'='result_view' THEN
        jsonb_build_object('budget',renderer,'admissions',jsonb_build_array(body#>'{source,admission}'),'release',NULL,
            'result_view_release',jsonb_build_array(9223372036854775807,body#>'{source,admission}',body#>'{source,execution}',body#>'{source,transaction}'))
    WHEN body#>>'{source,kind}'='wait' THEN
        jsonb_build_object('budget',renderer,'admissions',jsonb_build_array(body#>'{source,source_admission}'),
            'release',jsonb_build_array(body#>'{source,wait}',9223372036854775807,body#>'{source,prepared_transaction}'))
    ELSE jsonb_build_object('budget',renderer,'admissions','[]'::JSONB,'release',NULL)
        || CASE WHEN body#>'{source,frontier}' IS NULL THEN '{}'::JSONB ELSE
            jsonb_build_object('cache_frontier',jsonb_build_array(9223372036854775807,
                body#>'{source,frontier,predecessor}',body#>'{source,frontier,prepared_transaction}',
                body#>'{source,ordinal}',9223372036854775807,body#>'{source,frontier,prepared_transaction}'))
        END END
$$;
