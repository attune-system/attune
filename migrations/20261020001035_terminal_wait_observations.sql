-- Preserve registration custody for ordinary waits. A present observation of
-- an already-terminal row instead proves its own current transaction/release;
-- it cannot invent an earlier registration or reuse an older observation.
CREATE OR REPLACE FUNCTION protect_native_wait_completion_release()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
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
            LEFT JOIN runtime_native_wait_registration registration ON registration.wait = released.wait
            WHERE original.id = NEW.original_snapshot AND released.wait = NEW.wait
                AND original.body = source->'original'
                AND (
                    (registration.wait IS NOT NULL
                        AND registration.renderer = original.renderer_admission
                        AND registration.task_path = original.body->>'task_path'
                        AND registration.renderer = (source#>>'{registration,renderer}')::BIGINT
                        AND registration.selector IS NOT DISTINCT FROM (source#>>'{registration,selector}')::BIGINT
                        AND registration.task_path = source#>>'{registration,task_path}'
                        AND registration.recipe = source#>'{registration,recipe}'
                        AND registration.recipe_digest = source#>>'{registration,digest}'
                        AND registration.registration_transaction = source#>>'{registration,transaction}')
                    OR (registration.wait IS NULL
                        AND source->'registration' = 'null'::JSONB
                        AND original.body#>'{source,terminal_observation}' = 'true'::JSONB
                        AND original.body#>>'{source,state}' IN ('failed','timed_out')
                        AND released.release_transaction = NEW.release_transaction
                        AND original.body#>>'{source,prepared_transaction}' = NEW.release_transaction
                        AND EXISTS (
                            SELECT 1 FROM workflow_task_wait observed
                            WHERE observed.id = released.wait
                                AND observed.workflow_execution = NEW.workflow
                                AND observed.task_name = original.body#>>'{source,task_name}'
                                AND observed.state::TEXT = original.body#>>'{source,state}'
                                AND observed.result - '_delivery_complete' = original.body#>'{source,raw_projection}'
                        ))
                )
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
