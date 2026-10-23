-- Event identity is partition-qualified. Attribution does not keep executions,
-- admissions, identities or policies alive and never overloads sensor `source`.
CREATE TABLE execution_event_emitter (
    event BIGINT NOT NULL,
    event_created TIMESTAMPTZ NOT NULL,
    emitter JSONB NOT NULL CHECK (jsonb_typeof(emitter) = 'object'),
    emitter_digest TEXT NOT NULL CHECK (emitter_digest ~ '^[0-9a-f]{64}$'),
    PRIMARY KEY (event, event_created)
);
CREATE INDEX execution_event_emitter_cleanup_idx ON execution_event_emitter(event_created, event);
CREATE TRIGGER guard_execution_event_emitter BEFORE UPDATE ON execution_event_emitter
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
