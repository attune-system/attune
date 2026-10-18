-- Empty success has an original empty CONTROL, never a completed child.
CREATE TABLE runtime_native_empty_root_release (
    snapshot BIGINT PRIMARY KEY REFERENCES runtime_native_input_snapshot(id) ON DELETE CASCADE,
    execution BIGINT NOT NULL CHECK(execution > 0),
    workflow BIGINT NOT NULL CHECK(workflow > 0),
    renderer BIGINT NOT NULL REFERENCES runtime_authority_admission(id),
    release_transaction TEXT NOT NULL,
    recipe JSONB NOT NULL,
    recipe_digest TEXT NOT NULL,
    UNIQUE (execution, workflow)
);
CREATE TABLE runtime_native_empty_root_leaf (
    snapshot BIGINT NOT NULL REFERENCES runtime_native_empty_root_release(snapshot) ON DELETE CASCADE,
    receipt BIGINT NOT NULL REFERENCES runtime_input_receipt(id),
    PRIMARY KEY (snapshot, receipt)
);
CREATE INDEX runtime_native_empty_root_leaf_receipt ON runtime_native_empty_root_leaf(receipt);

CREATE FUNCTION protect_native_empty_root_release() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE original JSONB;
BEGIN
    IF TG_OP = 'UPDATE' THEN RAISE EXCEPTION 'Empty root release is immutable'; END IF;
    IF TG_OP = 'DELETE' THEN
        IF EXISTS(SELECT 1 FROM runtime_native_input_snapshot WHERE id=OLD.snapshot) THEN
            RAISE EXCEPTION 'Empty root release belongs to its retained snapshot';
        END IF;
        RETURN OLD;
    END IF;
    SELECT body INTO original FROM runtime_native_input_snapshot WHERE id=NEW.snapshot;
    IF original#>>'{source,kind}' IS DISTINCT FROM 'empty_iteration'
        OR original#>>'{source,transaction}' IS DISTINCT FROM pg_current_xact_id()::TEXT
        OR NEW.release_transaction IS DISTINCT FROM pg_current_xact_id()::TEXT
        OR NEW.renderer IS DISTINCT FROM (original->>'renderer')::BIGINT
        OR NEW.execution IS DISTINCT FROM (original#>>'{source,execution}')::BIGINT
        OR NEW.workflow IS DISTINCT FROM (original#>>'{source,workflow}')::BIGINT
        OR NEW.recipe->'executable' IS DISTINCT FROM original#>'{source,executable}'
        OR NEW.recipe->'contract' IS DISTINCT FROM original#>'{source,schema}'
        OR NOT EXISTS(SELECT 1 FROM runtime_native_empty_iteration_release
            WHERE snapshot=NEW.snapshot AND release_transaction=NEW.release_transaction)
    THEN RAISE EXCEPTION 'Empty root requires its original released empty Control'; END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER protect_native_empty_root_release BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_empty_root_release FOR EACH ROW EXECUTE FUNCTION protect_native_empty_root_release();

CREATE FUNCTION protect_native_empty_root_leaf() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' THEN RAISE EXCEPTION 'Empty root leaf is immutable'; END IF;
    IF TG_OP='DELETE' THEN
        IF EXISTS(SELECT 1 FROM runtime_native_empty_root_release WHERE snapshot=OLD.snapshot) THEN
            RAISE EXCEPTION 'Empty root leaf belongs to its retained release';
        END IF;
        RETURN OLD;
    END IF;
    IF NOT EXISTS(SELECT 1 FROM runtime_native_empty_root_release root
        JOIN runtime_input_receipt receipt ON receipt.id=NEW.receipt
        WHERE root.snapshot=NEW.snapshot AND root.release_transaction=pg_current_xact_id()::TEXT
            AND receipt.execution=root.execution AND receipt.renderer_admission=root.renderer
            AND receipt.entity_kind='execution_result' AND receipt.destination_purpose='value'
            AND receipt.destination_path <> '') THEN
        RAISE EXCEPTION 'Empty root leaf requires its original same-transaction receipt';
    END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER protect_native_empty_root_leaf BEFORE INSERT OR UPDATE OR DELETE
ON runtime_native_empty_root_leaf FOR EACH ROW EXECUTE FUNCTION protect_native_empty_root_leaf();
