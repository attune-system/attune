-- Forward-only extension: no migration checksum or stored executable is rewritten.
ALTER TABLE action ALTER COLUMN entrypoint DROP NOT NULL;
ALTER TABLE action ADD COLUMN launch JSONB;
ALTER TABLE action ADD CONSTRAINT action_execution_source_check
    CHECK ((entrypoint IS NULL) <> (launch IS NULL));
ALTER TABLE action ADD CONSTRAINT action_launch_shape_check
    CHECK (launch IS NULL OR (jsonb_typeof(launch) = 'object' AND workflow_def IS NULL));
ALTER TABLE action DROP CONSTRAINT action_parameter_delivery_check;
ALTER TABLE action ADD CONSTRAINT action_parameter_delivery_check
    CHECK (parameter_delivery IN ('stdin', 'file', 'none'));
ALTER TABLE action ADD CONSTRAINT action_no_input_launch_check
    CHECK (parameter_delivery <> 'none' OR
           (launch IS NOT NULL AND launch->>'type' IN ('command', 'native')));
COMMENT ON COLUMN action.launch IS 'Immutable-snapshotted typed process source; parameter CLI bindings live in param_schema';

CREATE TABLE execution_program_selection (
    id BIGSERIAL PRIMARY KEY,
    execution BIGINT NOT NULL,
    attempt UUID NOT NULL,
    worker BIGINT NOT NULL,
    release BIGINT NOT NULL,
    selection JSONB NOT NULL CHECK (jsonb_typeof(selection) = 'object'),
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(execution, attempt)
);
COMMENT ON TABLE execution_program_selection IS 'Attempt-bound executable metadata only; contains no argv, input values, credentials, or execution authority';
CREATE FUNCTION cleanup_execution_program_selection() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM execution_program_selection WHERE execution = OLD.id;
    RETURN OLD;
END;
$$;
CREATE TRIGGER cleanup_execution_program_selection AFTER DELETE ON execution
FOR EACH ROW EXECUTE FUNCTION cleanup_execution_program_selection();
