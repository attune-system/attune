-- Ordinary generated status/validation diagnostics have their own immutable facts.
CREATE TABLE runtime_native_success_metadata_observation (
    snapshot BIGINT PRIMARY KEY REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    observation JSONB NOT NULL CHECK(jsonb_typeof(observation)='object')
);
CREATE TABLE runtime_native_success_metadata_release (
    snapshot BIGINT PRIMARY KEY REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    execution BIGINT NOT NULL CHECK(execution>0),
    workflow BIGINT NOT NULL CHECK(workflow>0),
    completed_execution BIGINT NOT NULL CHECK(completed_execution>0),
    release_transaction TEXT NOT NULL
);
CREATE FUNCTION protect_native_success_metadata() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE prepared JSONB;
BEGIN
    IF TG_OP='UPDATE' THEN RAISE EXCEPTION 'Ordinary success facts are immutable'; END IF;
    IF TG_OP='DELETE' THEN
        IF EXISTS(SELECT 1 FROM runtime_native_input_snapshot WHERE id=OLD.snapshot) THEN
            RAISE EXCEPTION 'Success fact belongs to its retained snapshot';
        END IF;
        RETURN OLD;
    END IF;
    SELECT body->'source' INTO prepared FROM runtime_native_input_snapshot WHERE id=NEW.snapshot;
    IF prepared IS NULL OR prepared->>'kind' IS DISTINCT FROM 'success_metadata'
        OR prepared->>'transaction' IS DISTINCT FROM pg_current_xact_id()::TEXT THEN
        RAISE EXCEPTION 'Success requires the original issuing transaction';
    END IF;
    IF TG_TABLE_NAME='runtime_native_success_metadata_observation' THEN
        IF NEW.observation IS DISTINCT FROM (prepared - 'kind') THEN
            RAISE EXCEPTION 'Success observation differs from original source';
        END IF;
    ELSE
        IF NEW.execution IS DISTINCT FROM (prepared->>'execution')::BIGINT
            OR NEW.workflow IS DISTINCT FROM (prepared->>'workflow')::BIGINT
            OR NEW.completed_execution IS DISTINCT FROM (prepared->>'completed')::BIGINT
            OR NEW.release_transaction IS DISTINCT FROM prepared->>'transaction'
            OR NOT EXISTS(SELECT 1 FROM runtime_native_success_metadata_observation
                WHERE snapshot=NEW.snapshot AND observation=(prepared - 'kind'))
            OR NOT EXISTS(SELECT 1 FROM execution WHERE id=NEW.execution AND status::TEXT='running')
            OR NOT EXISTS(SELECT 1 FROM workflow_execution WHERE id=NEW.workflow AND execution=NEW.execution
                AND status::TEXT='running'
                AND to_jsonb(completed_tasks)=prepared->'projected_completed'
                AND cardinality(failed_tasks)=0 AND cardinality(current_tasks)=0 AND cardinality(skipped_tasks)=0) THEN
            RAISE EXCEPTION 'Success release requires actual projected state before terminal status';
        END IF;
    END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER protect_native_success_metadata_observation BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_success_metadata_observation FOR EACH ROW EXECUTE FUNCTION protect_native_success_metadata();
CREATE TRIGGER protect_native_success_metadata_release BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_success_metadata_release FOR EACH ROW EXECUTE FUNCTION protect_native_success_metadata();

-- Preserve the applied accounting implementation for every existing kind.
-- This helper is storage accounting only, not an alternate proof reader.
ALTER FUNCTION native_snapshot_storage_associations(JSONB,BIGINT)
RENAME TO native_snapshot_storage_associations_before_success;
CREATE FUNCTION native_snapshot_storage_associations(body JSONB,renderer BIGINT)
RETURNS JSONB LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT CASE WHEN body#>>'{source,kind}'='success_metadata' THEN
        jsonb_build_object('budget',renderer,'release',NULL,
            'admissions',(SELECT jsonb_agg(admission ORDER BY admission) FROM
                (SELECT renderer AS admission UNION SELECT (child->>'admission')::BIGINT
                 FROM jsonb_array_elements(body#>'{source,children}') child) sources),
            'success_metadata_observation',jsonb_build_array(9223372036854775807,(body->'source')-'kind'),
            'success_metadata_release',jsonb_build_array(9223372036854775807,body#>'{source,execution}',body#>'{source,workflow}',body#>'{source,completed}',body#>'{source,transaction}'))
    ELSE native_snapshot_storage_associations_before_success(body,renderer) END
$$;
