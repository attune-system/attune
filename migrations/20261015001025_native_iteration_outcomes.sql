-- Lifecycle observations of a complete, nonempty ordinary iteration. These are
-- not result VALUES, execution waits, projected cache EOF or synthetic children.
CREATE TABLE runtime_native_iteration_outcome_member (
    snapshot BIGINT NOT NULL REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal>=0),
    member JSONB NOT NULL CHECK(jsonb_typeof(member)='object'),
    PRIMARY KEY(snapshot,ordinal)
);
CREATE TABLE runtime_native_iteration_outcome_release (
    snapshot BIGINT PRIMARY KEY REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    execution BIGINT NOT NULL CHECK(execution>0),
    workflow BIGINT NOT NULL CHECK(workflow>0),
    release_transaction TEXT NOT NULL
);
CREATE FUNCTION protect_native_iteration_outcome() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE prepared JSONB;
BEGIN
    IF TG_OP='UPDATE' THEN RAISE EXCEPTION 'Native iteration observations are immutable'; END IF;
    IF TG_OP='DELETE' THEN
        IF EXISTS(SELECT 1 FROM runtime_native_input_snapshot WHERE id=OLD.snapshot) THEN
            RAISE EXCEPTION 'Native iteration observation belongs to its retained snapshot';
        END IF;
        RETURN OLD;
    END IF;
    SELECT body->'source' INTO prepared FROM runtime_native_input_snapshot WHERE id=NEW.snapshot;
    IF prepared IS NULL OR prepared->>'kind' IS DISTINCT FROM 'iteration_outcome'
        OR prepared->>'transaction' IS DISTINCT FROM pg_current_xact_id()::TEXT
        OR COALESCE(jsonb_array_length(prepared->'members'),0)=0 THEN
        RAISE EXCEPTION 'Native iteration observation requires its original issued source';
    END IF;
    IF TG_TABLE_NAME='runtime_native_iteration_outcome_member' THEN
        IF NEW.member IS DISTINCT FROM prepared->'members'->NEW.ordinal THEN
            RAISE EXCEPTION 'Native iteration member must match its original ordinal';
        END IF;
    ELSE
        IF NEW.execution IS DISTINCT FROM (prepared->>'execution')::BIGINT
            OR NEW.workflow IS DISTINCT FROM (prepared->>'workflow')::BIGINT
            OR NEW.release_transaction IS DISTINCT FROM prepared->>'transaction'
            OR (SELECT count(*) FROM runtime_native_iteration_outcome_member WHERE snapshot=NEW.snapshot)
                <>jsonb_array_length(prepared->'members') THEN
            RAISE EXCEPTION 'Native iteration release requires complete normalized membership';
        END IF;
    END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER protect_native_iteration_outcome_member BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_iteration_outcome_member FOR EACH ROW EXECUTE FUNCTION protect_native_iteration_outcome();
CREATE TRIGGER protect_native_iteration_outcome_release BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_iteration_outcome_release FOR EACH ROW EXECUTE FUNCTION protect_native_iteration_outcome();

CREATE OR REPLACE FUNCTION native_snapshot_storage_associations(body JSONB,renderer BIGINT)
RETURNS JSONB LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT CASE WHEN body#>>'{source,kind}'='iteration_outcome' THEN
        jsonb_build_object('budget',renderer,'release',NULL,
            'admissions',COALESCE((SELECT jsonb_agg(admission ORDER BY admission) FROM
                (SELECT DISTINCT (observation->>'admission')::BIGINT AS admission
                 FROM jsonb_array_elements(body#>'{source,members}') member,
                 LATERAL jsonb_array_elements(jsonb_build_array(member->'original',member->'latest')) observation) sources),'[]'::JSONB),
            'iteration_outcome_members',(SELECT jsonb_agg(jsonb_build_array(9223372036854775807,(member#>>'{original,ordinal}')::INTEGER,member) ORDER BY (member#>>'{original,ordinal}')::INTEGER)
                FROM jsonb_array_elements(body#>'{source,members}') member),
            'iteration_outcome_release',jsonb_build_array(9223372036854775807,body#>'{source,execution}',body#>'{source,workflow}',body#>'{source,transaction}'))
    WHEN body#>>'{source,kind}'='completion' THEN
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
