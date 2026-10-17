-- Real selector Controls reuse the existing issuing-TX staged receipt gate.
-- Their publication requires the actual normalized cache frontier, never an
-- assertion that a string selector was a Boolean decision or a published VALUE.
CREATE FUNCTION protect_native_cache_selector_publication() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
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
CREATE TRIGGER protect_native_cache_selector_publication
BEFORE UPDATE ON runtime_input_receipt_staged
FOR EACH ROW EXECUTE FUNCTION protect_native_cache_selector_publication();
