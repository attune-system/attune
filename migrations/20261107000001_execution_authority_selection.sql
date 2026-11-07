-- Upgrade is a coordinated authority cutover, not authority reconstruction.
-- Old rows did not persist a delegated decision or attempt nonce. Drain them
-- with the old binaries before upgrading; abort atomically rather than interrupt
-- running actions or reinterpret missing/revoked recorded evidence as delegation.
LOCK TABLE execution, enforcement, work_queue_item IN SHARE ROW EXCLUSIVE MODE;
DO $$
BEGIN
    IF EXISTS(SELECT 1 FROM execution e
              WHERE e.status IN ('requested','scheduling','scheduled','running','canceling')
                AND NOT EXISTS(SELECT 1 FROM runtime_authority_admission a
                               WHERE a.owner_kind='execution' AND a.owner_id=e.id))
       OR EXISTS(SELECT 1 FROM enforcement e WHERE e.status='created'
                 AND NOT EXISTS(SELECT 1 FROM runtime_authority_admission a
                                WHERE a.owner_kind='enforcement' AND a.owner_id=e.id))
       OR EXISTS(SELECT 1 FROM work_queue_item q WHERE q.status IN ('queued','leased','retry')
                 AND NOT EXISTS(SELECT 1 FROM runtime_authority_admission a
                                WHERE a.owner_kind='queue_item' AND a.owner_id=q.id))
    THEN
        RAISE EXCEPTION 'Authority cutover requires draining unrecorded executions, enforcements and queue items with the previous binaries; no rows have been converted';
    END IF;
END;
$$;

-- Server-owned execution authority, separate from executable ownership. Recorded
-- tombstones intentionally survive admission removal; no absence grants access.
CREATE TABLE execution_authority_selection (
    execution BIGINT PRIMARY KEY REFERENCES execution(id) ON DELETE CASCADE,
    mode TEXT NOT NULL CHECK (mode IN ('recorded','delegated','unavailable')),
    explicit BOOLEAN NOT NULL DEFAULT FALSE,
    owner BIGINT,
    selected_refs TEXT[] NOT NULL,
    executable_snapshot JSONB,
    schema_digest TEXT,
    grant_digest TEXT,
    output_grants JSONB,
    admission BIGINT,
    selection_transaction xid8 NOT NULL DEFAULT pg_current_xact_id(),
    CHECK ((mode = 'delegated') = explicit),
    CHECK (NOT explicit OR (owner IS NOT NULL AND executable_snapshot IS NOT NULL AND schema_digest IS NOT NULL AND grant_digest IS NOT NULL AND output_grants IS NOT NULL))
);
INSERT INTO execution_authority_selection(execution,mode,owner,selected_refs,executable_snapshot,admission)
SELECT e.id, CASE WHEN a.id IS NULL THEN 'unavailable' ELSE 'recorded' END,
       e.executor,e.permission_set_refs,e.executable_snapshot,a.id
FROM execution e LEFT JOIN runtime_authority_admission a ON a.owner_kind='execution' AND a.owner_id=e.id;

CREATE FUNCTION initialize_execution_authority_selection() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO execution_authority_selection(execution,mode,owner,selected_refs,executable_snapshot)
    VALUES(NEW.id, 'recorded',
           NEW.executor,NEW.permission_set_refs,NEW.executable_snapshot);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER initialize_execution_authority_selection AFTER INSERT ON execution
FOR EACH ROW EXECUTE FUNCTION initialize_execution_authority_selection();

CREATE FUNCTION guard_execution_authority_selection() RETURNS TRIGGER AS $$
BEGIN
    IF OLD.selection_transaction IS DISTINCT FROM pg_current_xact_id()
       OR NEW.selection_transaction IS DISTINCT FROM OLD.selection_transaction
       OR NEW.execution IS DISTINCT FROM OLD.execution
       OR OLD.admission IS NOT NULL
       OR OLD.explicit
       OR EXISTS(SELECT 1 FROM runtime_authority_admission WHERE owner_kind='execution' AND owner_id=OLD.execution)
    THEN RAISE EXCEPTION 'Execution authority selection is immutable'; END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER guard_execution_authority_selection BEFORE UPDATE ON execution_authority_selection
FOR EACH ROW EXECUTE FUNCTION guard_execution_authority_selection();

-- A retained enforcement must not become human-delegated after losing its
-- recorded admission, even if its source rule was originally ad hoc.
CREATE TABLE enforcement_recorded_authority (
    enforcement BIGINT PRIMARY KEY REFERENCES enforcement(id) ON DELETE CASCADE,
    admission BIGINT
);
INSERT INTO enforcement_recorded_authority(enforcement,admission)
SELECT e.id,a.id FROM enforcement e LEFT JOIN runtime_authority_admission a ON a.owner_kind='enforcement' AND a.owner_id=e.id;
CREATE TRIGGER enforcement_recorded_authority_immutable BEFORE UPDATE ON enforcement_recorded_authority
FOR EACH ROW EXECUTE FUNCTION guard_execution_runtime_read_origin();

-- Match-time human rule selection survives rule edits before dispatch.
CREATE TABLE delegated_enforcement_selection (
    enforcement BIGINT PRIMARY KEY REFERENCES enforcement(id) ON DELETE CASCADE,
    owner BIGINT NOT NULL,
    selected_refs TEXT[] NOT NULL,
    grant_digest TEXT NOT NULL,
    executable_snapshot JSONB NOT NULL,
    schema_digest TEXT NOT NULL
);
CREATE TRIGGER delegated_enforcement_selection_immutable BEFORE UPDATE ON delegated_enforcement_selection
FOR EACH ROW EXECUTE FUNCTION guard_execution_runtime_read_origin();

CREATE FUNCTION retain_recorded_execution_authority() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.owner_kind='execution' THEN
        IF EXISTS(SELECT 1 FROM execution_authority_selection WHERE execution=NEW.owner_id AND admission IS NOT NULL AND admission<>NEW.id)
        THEN RAISE EXCEPTION 'Existing recorded authority cannot be replaced'; END IF;
        -- An explicitly delegated creation can never acquire recorded authority.
        IF EXISTS(SELECT 1 FROM execution_authority_selection WHERE execution=NEW.owner_id AND explicit)
        THEN RAISE EXCEPTION 'Delegated execution cannot acquire recorded authority'; END IF;
        -- Insert has already happened, so this one-way recorded marker bypasses
        -- the selection guard through a distinct DELETE/INSERT, never downgrade.
        DELETE FROM execution_authority_selection WHERE execution=NEW.owner_id;
        INSERT INTO execution_authority_selection(execution,mode,owner,selected_refs,executable_snapshot,admission)
        SELECT id,'recorded',executor,permission_set_refs,executable_snapshot,NEW.id FROM execution WHERE id=NEW.owner_id;
    ELSIF NEW.owner_kind='enforcement' THEN
        IF EXISTS(SELECT 1 FROM delegated_enforcement_selection WHERE enforcement=NEW.owner_id)
        THEN RAISE EXCEPTION 'Delegated enforcement cannot acquire recorded authority'; END IF;
        IF EXISTS(SELECT 1 FROM enforcement_recorded_authority WHERE enforcement=NEW.owner_id AND admission IS DISTINCT FROM NEW.id)
        THEN RAISE EXCEPTION 'Existing recorded enforcement authority cannot be replaced'; END IF;
        INSERT INTO enforcement_recorded_authority(enforcement,admission) VALUES(NEW.owner_id,NEW.id) ON CONFLICT DO NOTHING;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER retain_recorded_execution_authority AFTER INSERT ON runtime_authority_admission
FOR EACH ROW EXECUTE FUNCTION retain_recorded_execution_authority();

CREATE TABLE delegated_execution_attempt (
    execution BIGINT PRIMARY KEY REFERENCES execution(id) ON DELETE CASCADE,
    attempt UUID NOT NULL,
    producer_kind TEXT NOT NULL DEFAULT 'worker' CHECK (producer_kind IN ('worker','workflow')),
    worker BIGINT,
    closed BOOLEAN NOT NULL DEFAULT FALSE,
    CHECK ((producer_kind='worker' AND worker IS NOT NULL)
        OR (producer_kind='workflow' AND worker IS NULL))
);
CREATE TABLE delegated_execution_key_origin (
    execution BIGINT NOT NULL REFERENCES execution(id) ON DELETE CASCADE,
    attempt UUID NOT NULL,
    origin_digest TEXT NOT NULL CHECK (origin_digest ~ '^[0-9a-f]{64}$'),
    origin JSONB NOT NULL CHECK (octet_length(origin::TEXT) <= 16384),
    PRIMARY KEY(execution,attempt,origin_digest)
);
CREATE TRIGGER delegated_execution_key_origin_immutable BEFORE UPDATE ON delegated_execution_key_origin
FOR EACH ROW EXECUTE FUNCTION guard_execution_runtime_read_origin();

CREATE FUNCTION guard_delegated_execution_attempt() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.execution IS DISTINCT FROM OLD.execution OR NEW.attempt IS DISTINCT FROM OLD.attempt
       OR NEW.producer_kind IS DISTINCT FROM OLD.producer_kind
       OR NEW.worker IS DISTINCT FROM OLD.worker OR OLD.closed AND NOT NEW.closed
    THEN RAISE EXCEPTION 'Delegated attempt identity and closure are immutable'; END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER guard_delegated_execution_attempt BEFORE UPDATE ON delegated_execution_attempt
FOR EACH ROW EXECUTE FUNCTION guard_delegated_execution_attempt();

-- Native workflows issue no worker/action credentials. Their producer fence is
-- created at delegated admission and closes atomically with terminal state,
-- including cancellation/timeout before orchestration has started.
CREATE FUNCTION close_terminal_delegated_attempt() RETURNS TRIGGER AS $$
BEGIN
    UPDATE delegated_execution_attempt SET closed=TRUE WHERE execution=NEW.id AND NOT closed;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER close_terminal_delegated_attempt AFTER UPDATE OF status ON execution
FOR EACH ROW WHEN (NEW.status IN ('completed','failed','cancelled','timeout','abandoned') AND OLD.status IS DISTINCT FROM NEW.status)
EXECUTE FUNCTION close_terminal_delegated_attempt();

-- Queue contributors retain their own human selection; batching cannot pick a
-- new permission list from mutable queue metadata or another contributor.
CREATE TABLE delegated_queue_item_selection (
    item BIGINT PRIMARY KEY REFERENCES work_queue_item(id) ON DELETE CASCADE,
    owner BIGINT NOT NULL,
    selected_refs TEXT[] NOT NULL,
    grant_digest TEXT NOT NULL
);
CREATE TRIGGER delegated_queue_item_selection_immutable BEFORE UPDATE ON delegated_queue_item_selection
FOR EACH ROW EXECUTE FUNCTION guard_execution_runtime_read_origin();
