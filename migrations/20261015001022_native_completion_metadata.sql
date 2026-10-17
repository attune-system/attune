CREATE TABLE runtime_native_completion_release (
    snapshot BIGINT PRIMARY KEY REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    execution BIGINT NOT NULL CHECK(execution>0),
    completed_execution BIGINT NOT NULL CHECK(completed_execution>0),
    release_transaction TEXT NOT NULL
);
CREATE FUNCTION protect_native_completion_release() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' THEN RAISE EXCEPTION 'Native completion release is immutable'; END IF;
    IF TG_OP='DELETE' THEN
        IF EXISTS(SELECT 1 FROM runtime_native_input_snapshot WHERE id=OLD.snapshot) THEN
            RAISE EXCEPTION 'Native completion release belongs to its retained snapshot';
        END IF;
        RETURN OLD;
    END IF;
    IF NEW.release_transaction<>pg_current_xact_id()::TEXT OR NOT EXISTS(
        SELECT 1 FROM runtime_native_input_snapshot snapshot
        WHERE snapshot.id=NEW.snapshot AND snapshot.body#>>'{source,kind}'='completion'
          AND snapshot.body#>>'{source,transaction}'=NEW.release_transaction
          AND (snapshot.body#>>'{source,execution}')::BIGINT=NEW.execution
          AND (snapshot.body#>>'{source,completed}')::BIGINT=NEW.completed_execution
    ) THEN RAISE EXCEPTION 'Native completion release must match its actual prepared state'; END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER protect_native_completion_release BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_completion_release FOR EACH ROW EXECUTE FUNCTION protect_native_completion_release();

CREATE OR REPLACE FUNCTION native_snapshot_storage_associations(body JSONB,renderer BIGINT)
RETURNS JSONB LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT CASE WHEN body#>>'{source,kind}'='completion' THEN
        jsonb_build_object('budget',renderer,'admissions',COALESCE((SELECT jsonb_agg(admission ORDER BY admission) FROM
            (SELECT DISTINCT (child->>'admission')::BIGINT AS admission FROM jsonb_array_elements(body#>'{source,children}') child) sources),'[]'::JSONB),
            'release',NULL,'completion_release',jsonb_build_array(9223372036854775807,body#>'{source,execution}',body#>'{source,completed}',body#>'{source,transaction}'))
    WHEN body#>>'{source,kind}'='result_view' THEN
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
