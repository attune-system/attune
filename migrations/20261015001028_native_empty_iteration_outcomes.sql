-- Constructive ordinary empty-list outcomes. No rows in the nonempty group
-- member/release tables are inserted by this recipe.
CREATE TABLE runtime_native_empty_iteration_release (
    snapshot BIGINT PRIMARY KEY REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    execution BIGINT NOT NULL CHECK(execution > 0),
    workflow BIGINT NOT NULL CHECK(workflow > 0),
    control BIGINT NOT NULL REFERENCES runtime_input_receipt(id),
    release_transaction TEXT NOT NULL
);

CREATE FUNCTION protect_native_empty_iteration_release() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE source JSONB;
BEGIN
    IF TG_OP = 'UPDATE' THEN RAISE EXCEPTION 'Native empty iteration release is immutable'; END IF;
    IF TG_OP = 'DELETE' THEN
        IF EXISTS(SELECT 1 FROM runtime_native_input_snapshot WHERE id = OLD.snapshot) THEN
            RAISE EXCEPTION 'Native empty iteration release belongs to its retained snapshot';
        END IF;
        RETURN OLD;
    END IF;
    SELECT body->'source' INTO source FROM runtime_native_input_snapshot WHERE id = NEW.snapshot;
    IF source->>'kind' IS DISTINCT FROM 'empty_iteration'
        OR source->>'transaction' IS DISTINCT FROM pg_current_xact_id()::TEXT
        OR NEW.release_transaction IS DISTINCT FROM pg_current_xact_id()::TEXT
        OR NEW.execution IS DISTINCT FROM (source->>'execution')::BIGINT
        OR NEW.workflow IS DISTINCT FROM (source->>'workflow')::BIGINT
        OR NEW.control IS DISTINCT FROM (source->>'control')::BIGINT
        OR NOT EXISTS(SELECT 1 FROM runtime_native_iteration_control binding
            JOIN runtime_input_receipt_staged stage ON stage.receipt = binding.receipt
            WHERE binding.receipt = NEW.control AND binding.execution = NEW.execution
                AND binding.workflow = NEW.workflow
                AND binding.task_path = source#>>'{control_site,path}'
                AND binding.prepared_transaction = NEW.release_transaction
                AND binding.publication_transaction = NEW.release_transaction
                AND stage.prepared_transaction = NEW.release_transaction
                AND stage.publication_transaction = NEW.release_transaction)
        OR EXISTS(SELECT 1 FROM workflow_task_dispatch
            WHERE workflow_execution = NEW.workflow AND task_name = source->>'task_name')
        OR EXISTS(SELECT 1 FROM execution
            WHERE parent = NEW.execution AND workflow_task->>'task_name' = source->>'task_name')
    THEN RAISE EXCEPTION 'Native empty iteration release requires its original finalized zero-dispatch Control';
    END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER protect_native_empty_iteration_release BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_empty_iteration_release FOR EACH ROW EXECUTE FUNCTION protect_native_empty_iteration_release();

-- Preserve all previously installed recipes, including 01025/01026/01027.
ALTER FUNCTION native_snapshot_storage_associations(JSONB, BIGINT)
    RENAME TO native_snapshot_storage_associations_before_empty_iteration;
CREATE FUNCTION native_snapshot_storage_associations(body JSONB, renderer BIGINT)
RETURNS JSONB LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT CASE WHEN body#>>'{source,kind}' = 'empty_iteration' THEN
        jsonb_build_object('budget',renderer,'admissions','[]'::JSONB,'release',NULL,
            'empty_iteration_release',jsonb_build_array(9223372036854775807,
                body#>'{source,execution}',body#>'{source,workflow}',body#>'{source,control}',body#>'{source,transaction}'))
    ELSE native_snapshot_storage_associations_before_empty_iteration(body,renderer) END
$$;
