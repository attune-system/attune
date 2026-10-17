-- A real ordinary with_items Control may depend on same-advance VALUE stages.
-- It binds to its actual workflow/site, never to a fabricated cache frontier.
CREATE TABLE runtime_native_iteration_control (
    receipt BIGINT PRIMARY KEY REFERENCES runtime_input_receipt(id) ON DELETE CASCADE,
    execution BIGINT NOT NULL CHECK(execution>0),
    workflow BIGINT NOT NULL CHECK(workflow>0),
    workflow_definition BIGINT NOT NULL CHECK(workflow_definition>0),
    task_path TEXT NOT NULL CHECK(task_path ~ '^/workflow/tasks/[^/]+(/tasks/[^/]+)*/with_items$'),
    batch_size BIGINT CHECK(batch_size>0),
    prepared_transaction TEXT NOT NULL DEFAULT pg_current_xact_id()::TEXT,
    publication_transaction TEXT,
    CHECK(publication_transaction IS NULL OR publication_transaction=prepared_transaction)
);
CREATE FUNCTION protect_native_iteration_control() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF EXISTS(SELECT 1 FROM runtime_input_receipt WHERE id=OLD.receipt) THEN
            RAISE EXCEPTION 'Native iteration Control belongs to its retained receipt';
        END IF;
        RETURN OLD;
    END IF;
    IF TG_OP='INSERT' THEN
        IF NEW.prepared_transaction<>pg_current_xact_id()::TEXT OR NEW.publication_transaction IS NOT NULL THEN
            RAISE EXCEPTION 'Native iteration Control must start unpublished in its issuing transaction';
        END IF;
    ELSE
        IF NEW.receipt IS DISTINCT FROM OLD.receipt OR NEW.execution IS DISTINCT FROM OLD.execution
            OR NEW.workflow IS DISTINCT FROM OLD.workflow OR NEW.task_path IS DISTINCT FROM OLD.task_path
            OR NEW.workflow_definition IS DISTINCT FROM OLD.workflow_definition
            OR NEW.batch_size IS DISTINCT FROM OLD.batch_size
            OR NEW.prepared_transaction IS DISTINCT FROM OLD.prepared_transaction
            OR OLD.publication_transaction IS NOT NULL
            OR NEW.publication_transaction IS DISTINCT FROM pg_current_xact_id()::TEXT
            OR NEW.prepared_transaction<>pg_current_xact_id()::TEXT THEN
            RAISE EXCEPTION 'Native iteration Control binding is immutable and one-use';
        END IF;
        IF NOT EXISTS(SELECT 1 FROM runtime_input_receipt_staged WHERE receipt=NEW.receipt
            AND prepared_transaction=NEW.prepared_transaction AND publication_transaction IS NULL)
            OR EXISTS(SELECT 1 FROM runtime_input_receipt_source parent
                JOIN runtime_input_receipt_staged stage ON stage.receipt=parent.source_receipt
                WHERE parent.receipt=NEW.receipt AND stage.publication_transaction IS DISTINCT FROM stage.prepared_transaction) THEN
            RAISE EXCEPTION 'Native iteration Control awaits its actual upstream publications';
        END IF;
    END IF;
    IF NOT EXISTS(
        SELECT 1 FROM runtime_input_receipt receipt
        JOIN runtime_authority_admission admission ON admission.id=receipt.renderer_admission
        JOIN workflow_execution workflow ON workflow.id=NEW.workflow AND workflow.execution=NEW.execution AND workflow.workflow_def=NEW.workflow_definition
        LEFT JOIN runtime_declared_input_binding declaration ON declaration.id=receipt.declaration
        WHERE receipt.id=NEW.receipt AND receipt.execution=NEW.execution
          AND receipt.renderer_admission=receipt.destination_admission
          AND admission.owner_kind='execution' AND admission.owner_id=NEW.execution
          AND receipt.entity_kind='workflow_variables' AND receipt.destination_purpose='control'
          AND receipt.destination_path=NEW.task_path AND NEW.task_path LIKE '/workflow/tasks/%/with_items'
          AND COALESCE(declaration.declaration->>'source_path',receipt.proof#>>'{literal,path}')=NEW.task_path
    ) THEN RAISE EXCEPTION 'Native iteration Control requires its actual retained with_items site and workflow'; END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER protect_native_iteration_control BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_iteration_control FOR EACH ROW EXECUTE FUNCTION protect_native_iteration_control();

-- Keep the validated cache guard intact for cache selectors. Only the distinct,
-- normalized ordinary with_items binding can satisfy the new branch.
CREATE OR REPLACE FUNCTION protect_native_cache_selector_publication() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
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
