-- A distinct task-metadata CONTROL binding. It is neither a cache selector nor
-- a with_items list. Its publication belongs to a real attributed dispatch.
CREATE TABLE runtime_native_empty_task_control (
    receipt BIGINT PRIMARY KEY REFERENCES runtime_input_receipt(id) ON DELETE CASCADE,
    execution BIGINT NOT NULL CHECK(execution>0),
    workflow BIGINT NOT NULL CHECK(workflow>0),
    workflow_definition BIGINT NOT NULL CHECK(workflow_definition>0),
    source_snapshot BIGINT NOT NULL REFERENCES runtime_native_input_snapshot(id),
    task_name TEXT NOT NULL CHECK(task_name<>''),
    task_path TEXT NOT NULL CHECK(task_path ~ '^/workflow/tasks/[^/]+(/tasks/[^/]+)*$'),
    child BIGINT CHECK(child>0),
    prepared_transaction TEXT NOT NULL DEFAULT pg_current_xact_id()::TEXT,
    publication_transaction TEXT,
    CHECK(publication_transaction IS NULL AND child IS NULL
        OR publication_transaction IS NOT NULL AND publication_transaction=prepared_transaction AND child IS NOT NULL)
);

CREATE FUNCTION protect_native_empty_task_control() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE source JSONB; target JSONB;
BEGIN
    IF TG_OP='DELETE' THEN
        IF EXISTS(SELECT 1 FROM runtime_input_receipt WHERE id=OLD.receipt) THEN
            RAISE EXCEPTION 'Empty task metadata CONTROL belongs to its retained receipt';
        END IF;
        RETURN OLD;
    END IF;
    IF NEW.prepared_transaction<>pg_current_xact_id()::TEXT THEN
        RAISE EXCEPTION 'Empty task metadata CONTROL requires its issuing transaction';
    END IF;
    IF TG_OP='INSERT' THEN
        IF NEW.publication_transaction IS NOT NULL OR NEW.child IS NOT NULL THEN
            RAISE EXCEPTION 'Empty task metadata CONTROL must start unpublished';
        END IF;
    ELSIF NEW.receipt IS DISTINCT FROM OLD.receipt
        OR NEW.execution IS DISTINCT FROM OLD.execution
        OR NEW.workflow IS DISTINCT FROM OLD.workflow
        OR NEW.workflow_definition IS DISTINCT FROM OLD.workflow_definition
        OR NEW.source_snapshot IS DISTINCT FROM OLD.source_snapshot
        OR NEW.task_name IS DISTINCT FROM OLD.task_name
        OR NEW.task_path IS DISTINCT FROM OLD.task_path
        OR NEW.prepared_transaction IS DISTINCT FROM OLD.prepared_transaction
        OR OLD.publication_transaction IS NOT NULL OR OLD.child IS NOT NULL
        OR NEW.publication_transaction IS DISTINCT FROM pg_current_xact_id()::TEXT
        OR NEW.child IS NULL THEN
        RAISE EXCEPTION 'Empty task metadata CONTROL binding is immutable and one-use';
    END IF;
    SELECT body->'source' INTO source FROM runtime_native_input_snapshot
        WHERE id=NEW.source_snapshot;
    target := (source#>'{executable,executable,workflow_definition,definition}')
        #> string_to_array(substr(NEW.task_path,length('/workflow/')+1),'/');
    IF source->>'kind' IS DISTINCT FROM 'empty_iteration'
        OR source->>'execution' IS DISTINCT FROM NEW.execution::TEXT
        OR source->>'workflow' IS DISTINCT FROM NEW.workflow::TEXT
        OR source->>'definition' IS DISTINCT FROM NEW.workflow_definition::TEXT
        OR source->>'transaction' IS DISTINCT FROM NEW.prepared_transaction
        OR target->>'name' IS DISTINCT FROM NEW.task_name
        OR NOT EXISTS(SELECT 1 FROM runtime_input_receipt receipt
            JOIN runtime_input_receipt_staged stage ON stage.receipt=receipt.id
            JOIN runtime_authority_admission admission ON admission.id=receipt.renderer_admission
            JOIN runtime_declared_input_binding declaration ON declaration.id=receipt.declaration
                AND declaration.policy_revision=admission.policy_revision
            JOIN runtime_native_input_snapshot snapshot ON snapshot.id=NEW.source_snapshot
                AND snapshot.renderer_admission=receipt.renderer_admission
            JOIN runtime_input_receipt_snapshot edge ON edge.receipt=receipt.id
                AND edge.snapshot=snapshot.id
            JOIN workflow_execution workflow ON workflow.id=NEW.workflow
                AND workflow.execution=NEW.execution AND workflow.workflow_def=NEW.workflow_definition
            WHERE receipt.id=NEW.receipt AND receipt.execution=NEW.execution
                AND receipt.renderer_admission=receipt.destination_admission
                AND admission.owner_kind='execution' AND admission.owner_id=NEW.execution
                AND receipt.entity_kind='workflow_variables' AND receipt.destination_purpose='control'
                AND receipt.destination_path LIKE NEW.task_path || '/%'
                AND receipt.destination_path ~ '^/workflow/tasks/[^/]+(/tasks/[^/]+)*/(action|permission_set_refs|timeout|worker_selector|worker_tolerations|worker_affinity|trace_tag_template)(/.*)?$'
                AND declaration.declaration->>'source_path'=receipt.destination_path
                AND stage.prepared_transaction=NEW.prepared_transaction
                AND stage.publication_transaction IS NULL) THEN
        RAISE EXCEPTION 'Empty task metadata CONTROL requires its actual declaration, workflow and original empty source';
    END IF;
    IF TG_OP='UPDATE' THEN
        IF NOT EXISTS(SELECT 1 FROM runtime_native_empty_iteration_release released
                WHERE released.snapshot=NEW.source_snapshot AND released.execution=NEW.execution
                    AND released.workflow=NEW.workflow AND released.release_transaction=NEW.publication_transaction)
            OR NOT EXISTS(SELECT 1 FROM workflow_task_dispatch dispatch
                JOIN execution child ON child.id=dispatch.execution_id
                WHERE dispatch.workflow_execution=NEW.workflow AND dispatch.task_name=NEW.task_name
                    AND dispatch.task_index IS NULL AND child.id=NEW.child
                    AND child.parent=NEW.execution AND child.original_execution IS NULL
                    AND child.status='requested' AND child.config IS NULL
                    AND child.workflow_task->>'workflow_execution'=NEW.workflow::TEXT
                    AND child.workflow_task->>'task_name'=NEW.task_name
                    AND child.workflow_task->>'triggered_by'=source->>'task_name'
                    AND child.workflow_task->>'task_index' IS NULL
                    AND child.workflow_task->>'task_batch' IS NULL
                    AND child.workflow_task->>'retry_count'='0')
            OR EXISTS(SELECT 1 FROM runtime_input_receipt_source parent
                JOIN runtime_input_receipt_staged stage ON stage.receipt=parent.source_receipt
                WHERE parent.receipt=NEW.receipt
                    AND stage.publication_transaction IS DISTINCT FROM stage.prepared_transaction) THEN
            RAISE EXCEPTION 'Empty task metadata CONTROL awaits its actual release, dispatch and upstream publications';
        END IF;
    END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER protect_native_empty_task_control BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_empty_task_control FOR EACH ROW EXECUTE FUNCTION protect_native_empty_task_control();

-- Preserve the 01024 iteration and cache branches unchanged. Only the distinct
-- normalized metadata binding can satisfy this additional publication branch.
CREATE OR REPLACE FUNCTION protect_native_cache_selector_publication() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM runtime_native_empty_task_control metadata
        JOIN runtime_input_receipt receipt ON receipt.id=metadata.receipt
        JOIN runtime_native_empty_iteration_release released ON released.snapshot=metadata.source_snapshot
        WHERE metadata.receipt=NEW.receipt AND metadata.execution=receipt.execution
            AND receipt.entity_kind='workflow_variables' AND receipt.destination_purpose='control'
            AND metadata.child>0 AND metadata.prepared_transaction=NEW.prepared_transaction
            AND metadata.publication_transaction=NEW.publication_transaction
            AND released.execution=metadata.execution AND released.workflow=metadata.workflow
            AND released.release_transaction=NEW.publication_transaction
            AND NEW.publication_transaction=pg_current_xact_id()::TEXT) THEN RETURN NEW; END IF;
    IF EXISTS(SELECT 1 FROM runtime_native_iteration_control ordinary
        JOIN runtime_input_receipt receipt ON receipt.id=ordinary.receipt
        WHERE ordinary.receipt=NEW.receipt AND ordinary.execution=receipt.execution
          AND ordinary.task_path=receipt.destination_path
          AND receipt.entity_kind='workflow_variables' AND receipt.destination_purpose='control'
          AND ordinary.prepared_transaction=NEW.prepared_transaction
          AND ordinary.publication_transaction=NEW.publication_transaction
          AND NEW.publication_transaction=pg_current_xact_id()::TEXT) THEN RETURN NEW; END IF;
    IF EXISTS(SELECT 1 FROM runtime_input_receipt WHERE id=NEW.receipt AND destination_purpose='control')
        AND NOT EXISTS(
            SELECT 1 FROM runtime_input_receipt receipt
            JOIN runtime_native_input_snapshot_parent parent ON parent.receipt=receipt.id
            JOIN runtime_native_input_snapshot snapshot ON snapshot.id=parent.snapshot
            JOIN runtime_native_cache_frontier frontier ON frontier.snapshot=snapshot.id
            WHERE receipt.id=NEW.receipt
              AND snapshot.body#>>'{source,kind}'='cache'
              AND receipt.entity_kind='workflow_variables'
              AND receipt.renderer_admission=snapshot.renderer_admission
              AND receipt.destination_path IN (
                  (snapshot.body->>'task_path') || '/iterate_cache/namespace',
                  (snapshot.body->>'task_path') || '/iterate_cache/generation',
                  (snapshot.body->>'task_path') || '/iterate_cache/owner_type',
                  (snapshot.body->>'task_path') || '/iterate_cache/owner_ref')
              AND frontier.bound_iteration>0
              AND frontier.prepared_transaction=NEW.prepared_transaction
              AND frontier.bound_transaction=NEW.publication_transaction
              AND NEW.publication_transaction=pg_current_xact_id()::TEXT
        ) THEN
        RAISE EXCEPTION 'Native cache selector publication requires its actual bound frontier';
    END IF;
    RETURN NEW;
END; $$;
