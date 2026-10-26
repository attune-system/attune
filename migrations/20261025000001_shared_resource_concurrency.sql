ALTER TABLE policy ADD COLUMN resource_concurrency JSONB;
ALTER TABLE policy DROP CONSTRAINT policy_has_feature;
ALTER TABLE policy ADD CONSTRAINT policy_has_feature CHECK (
    threshold IS NOT NULL OR rate_limit_max_executions IS NOT NULL
    OR jsonb_array_length(quotas) > 0 OR resource_concurrency IS NOT NULL
);
ALTER TABLE policy ADD CONSTRAINT resource_policy_separate_controls CHECK (
    resource_concurrency IS NULL OR (
        action IS NULL AND threshold IS NULL AND method IS NULL
        AND rate_limit_max_executions IS NULL AND quotas = '[]'::jsonb
        AND jsonb_typeof(resource_concurrency) = 'object'
        AND (resource_concurrency->>'limit')::integer > 0
        AND resource_concurrency->>'method' IN ('enqueue', 'cancel')
        AND length(resource_concurrency->>'namespace') > 0
        AND jsonb_array_length(resource_concurrency->'participants') > 0
    )
);

-- Unique membership is enforced by PostgreSQL, including simultaneous updates
-- and enabled_override changes made outside the policy API.
CREATE TABLE resource_concurrency_participant (
    action_ref TEXT PRIMARY KEY,
    policy_id BIGINT NOT NULL REFERENCES policy(id) ON DELETE CASCADE,
    resource_parameter TEXT NOT NULL
);
CREATE INDEX resource_concurrency_participant_policy ON resource_concurrency_participant (policy_id);

CREATE TABLE execution_resource_admission (
    execution_id BIGINT PRIMARY KEY REFERENCES execution(id) ON DELETE CASCADE,
    policy_id BIGINT NOT NULL REFERENCES policy(id) ON DELETE RESTRICT,
    resource_group TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT false,
    queue_order BIGSERIAL NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX execution_resource_admission_group ON execution_resource_admission
    (policy_id, resource_group, active, queue_order);

CREATE FUNCTION sync_resource_policy() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.resource_concurrency IS DISTINCT FROM OLD.resource_concurrency
       AND EXISTS (SELECT 1 FROM execution_resource_admission WHERE policy_id = OLD.id) THEN
        RAISE EXCEPTION 'Resource policy cannot be edited while executions are active or waiting' USING ERRCODE = '23514';
    END IF;
    DELETE FROM resource_concurrency_participant WHERE policy_id = NEW.id;
    IF NEW.effective_enabled AND NEW.retired_at IS NULL AND NEW.resource_concurrency IS NOT NULL THEN
        IF EXISTS (
            SELECT 1 FROM jsonb_array_elements(NEW.resource_concurrency->'participants') item
            LEFT JOIN action a ON a.ref = item->>'action' WHERE a.id IS NULL
        ) THEN
            RAISE EXCEPTION 'Shared resource participants must reference existing actions' USING ERRCODE = '23514';
        END IF;
        INSERT INTO resource_concurrency_participant (action_ref, policy_id, resource_parameter)
        SELECT item->>'action', NEW.id, item->>'resource_parameter'
        FROM jsonb_array_elements(NEW.resource_concurrency->'participants') item;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER sync_resource_policy AFTER INSERT OR UPDATE OF resource_concurrency, enabled, enabled_override, retired_at
    ON policy FOR EACH ROW EXECUTE FUNCTION sync_resource_policy();

-- Terminal state is the authoritative release boundary, not message delivery.
-- Canceling retains capacity. Abandoned releases it but does not fence external IO.
CREATE FUNCTION release_terminal_resource_admission() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE released RECORD; policy_ref TEXT;
BEGIN
    IF NEW.status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned') THEN
        FOR released IN DELETE FROM execution_resource_admission WHERE execution_id = NEW.id RETURNING policy_id, resource_group LOOP
            SELECT ref INTO policy_ref FROM policy WHERE id = released.policy_id;
            RAISE LOG 'Shared resource reservation released policy_ref=% resource_group=% execution_id=% terminal_state=%', policy_ref, released.resource_group, NEW.id, NEW.status;
        END LOOP;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER release_terminal_resource_admission AFTER UPDATE OF status ON execution
    FOR EACH ROW EXECUTE FUNCTION release_terminal_resource_admission();

-- A hint for the existing supervisor republisher, never an admission decision.
-- The executor rechecks capacity under locks. No waiting reservation is active.
CREATE FUNCTION shared_resource_waiter_ready(owner_id BIGINT) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM execution_resource_admission request
        JOIN policy p ON p.id = request.policy_id
        WHERE request.execution_id = owner_id AND NOT request.active
          AND (NOT p.effective_enabled OR p.retired_at IS NOT NULL OR (
            (SELECT count(*) FROM execution_resource_admission occupied
             WHERE occupied.policy_id = request.policy_id
               AND occupied.resource_group = request.resource_group AND occupied.active)
                < (p.resource_concurrency->>'limit')::integer
            AND NOT EXISTS (
                SELECT 1 FROM execution_resource_admission earlier
                WHERE earlier.policy_id = request.policy_id
                  AND earlier.resource_group = request.resource_group
                  AND NOT earlier.active AND earlier.queue_order < request.queue_order
            )
          ))
          AND NOT EXISTS (
            SELECT 1 FROM execution_admission_entry entry
            JOIN execution_admission_state state ON state.id = entry.state_id
            WHERE entry.execution_id = owner_id AND entry.status = 'queued'
              AND ((SELECT count(*) FROM execution_admission_entry occupied
                    WHERE occupied.state_id = state.id AND occupied.status = 'active') >= state.max_concurrent
                   OR EXISTS (SELECT 1 FROM execution_admission_entry earlier
                              WHERE earlier.state_id = state.id AND earlier.status = 'queued'
                                AND earlier.queue_order < entry.queue_order))
          )
    )
$$;
