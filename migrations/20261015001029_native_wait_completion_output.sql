-- Only the owning output snapshot has an FK. Runtime identities and the
-- original wait snapshot keep independent-retention/custody graph semantics.
CREATE TABLE runtime_native_wait_completion_release (
    snapshot BIGINT PRIMARY KEY REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    execution BIGINT NOT NULL CHECK (execution > 0),
    workflow BIGINT NOT NULL CHECK (workflow > 0),
    wait BIGINT NOT NULL CHECK (wait > 0),
    original_snapshot BIGINT NOT NULL CHECK (original_snapshot > 0 AND original_snapshot < snapshot),
    release_transaction TEXT NOT NULL
);

CREATE FUNCTION protect_native_wait_completion_release() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE source JSONB;
BEGIN
    IF TG_OP = 'UPDATE' THEN RAISE EXCEPTION 'Wait completion release is immutable'; END IF;
    IF TG_OP = 'DELETE' THEN
        IF EXISTS (SELECT 1 FROM runtime_native_input_snapshot WHERE id = OLD.snapshot) THEN
            RAISE EXCEPTION 'Wait completion release belongs to its retained snapshot';
        END IF;
        RETURN OLD;
    END IF;
    SELECT body->'source' INTO source FROM runtime_native_input_snapshot
    WHERE id = NEW.snapshot AND body#>>'{source,kind}' = 'wait_completion';
    IF source IS NULL OR NEW.release_transaction IS DISTINCT FROM pg_current_xact_id()::TEXT
        OR source->>'transaction' IS DISTINCT FROM NEW.release_transaction
        OR (source->>'execution')::BIGINT IS DISTINCT FROM NEW.execution
        OR (source->>'workflow')::BIGINT IS DISTINCT FROM NEW.workflow
        OR (source->>'original_snapshot')::BIGINT IS DISTINCT FROM NEW.original_snapshot
        OR (source#>>'{original,source,wait}')::BIGINT IS DISTINCT FROM NEW.wait
        OR source#>>'{original,source,kind}' IS DISTINCT FROM 'wait'
        OR NOT EXISTS (
            SELECT 1 FROM runtime_native_input_snapshot original
            JOIN runtime_native_wait_release released ON released.snapshot = original.id
            JOIN runtime_native_wait_registration registration ON registration.wait = released.wait
            WHERE original.id = NEW.original_snapshot AND released.wait = NEW.wait
                AND original.body = source->'original'
                AND registration.renderer = original.renderer_admission
                AND registration.task_path = original.body->>'task_path'
                AND registration.renderer = (source#>>'{registration,renderer}')::BIGINT
                AND registration.selector IS NOT DISTINCT FROM (source#>>'{registration,selector}')::BIGINT
                AND registration.task_path = source#>>'{registration,task_path}'
                AND registration.recipe = source#>'{registration,recipe}'
                AND registration.recipe_digest = source#>>'{registration,digest}'
                AND registration.registration_transaction = source#>>'{registration,transaction}'
        )
        OR NOT EXISTS (
            SELECT 1 FROM execution parent JOIN workflow_execution workflow ON workflow.execution = parent.id
            WHERE parent.id = NEW.execution AND workflow.id = NEW.workflow
                AND parent.status = 'running' AND workflow.status = 'running' AND NOT workflow.paused
                AND parent.result IS NOT DISTINCT FROM NULLIF(source#>'{parent,result}', 'null'::JSONB)
                AND parent.updated = (source#>>'{parent,updated}')::TIMESTAMPTZ
                AND to_jsonb(workflow.current_tasks) = source#>'{projected,current_tasks}'
                AND to_jsonb(workflow.completed_tasks) = source#>'{projected,completed_tasks}'
                AND to_jsonb(workflow.failed_tasks) = source#>'{projected,failed_tasks}'
                AND to_jsonb(workflow.skipped_tasks) = source#>'{projected,skipped_tasks}'
                AND workflow.variables = source#>'{projected,variables}'
                AND workflow.updated = (source#>>'{projected,updated}')::TIMESTAMPTZ
        )
        OR EXISTS (
            SELECT 1 FROM runtime_native_input_snapshot_parent edge
            JOIN runtime_input_receipt_staged staged ON staged.receipt = edge.receipt
            WHERE edge.snapshot = NEW.snapshot AND staged.publication_transaction IS DISTINCT FROM staged.prepared_transaction
        )
    THEN RAISE EXCEPTION 'Wait completion requires its actual published wait and exact running target'; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER protect_native_wait_completion_release BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_wait_completion_release FOR EACH ROW EXECUTE FUNCTION protect_native_wait_completion_release();

-- Preserve every existing kind's applied accounting implementation, including
-- independently integrated cache/success/list recipes.
ALTER FUNCTION native_snapshot_storage_associations(JSONB, BIGINT)
    RENAME TO native_snapshot_storage_associations_before_wait_completion;
CREATE FUNCTION native_snapshot_storage_associations(body JSONB, renderer BIGINT)
RETURNS JSONB LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT CASE WHEN body#>>'{source,kind}' = 'wait_completion' THEN
        jsonb_build_object('budget', renderer, 'admissions',
            (SELECT jsonb_agg(admission ORDER BY admission) FROM (
                SELECT DISTINCT admission FROM (VALUES
                    ((body#>>'{source,original,renderer}')::BIGINT),
                    ((body#>>'{source,original,source,source_admission}')::BIGINT)
                ) ids(admission)
            ) distinct_ids),
            'release', NULL, 'wait_completion_release', jsonb_build_array(
                9223372036854775807, body#>'{source,execution}', body#>'{source,workflow}',
                body#>'{source,original,source,wait}', body#>'{source,original_snapshot}', body#>'{source,transaction}'))
    ELSE native_snapshot_storage_associations_before_wait_completion(body, renderer) END
$$;
