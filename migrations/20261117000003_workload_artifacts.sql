-- Private filesystem output has its own publication authority. It is not a
-- runtime-stage output or an execution-wide terminal upload capability.
-- Source/attempt references intentionally have no retention-coupling FKs.
CREATE TABLE workload_artifact_publication (
    id BIGSERIAL PRIMARY KEY,
    launch BIGINT NOT NULL UNIQUE,
    max_total_bytes BIGINT NOT NULL DEFAULT 268435456 CHECK (max_total_bytes>0),
    max_entries BIGINT NOT NULL DEFAULT 4096 CHECK (max_entries>0),
    state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','pending','publishing','complete','incomplete')),
    closed_at TIMESTAMPTZ,
    deadline TIMESTAMPTZ,
    generation BIGINT NOT NULL DEFAULT 0 CHECK (generation>=0),
    controller BIGINT,
    controller_incarnation UUID,
    lease_id UUID,
    lease_hash TEXT,
    lease_deadline TIMESTAMPTZ,
    cleanup_receipt BIGINT,
    CHECK ((closed_at IS NULL)=(state='open')),
    CHECK ((deadline IS NULL)=(state='open')),
    CHECK (cleanup_receipt IS NULL OR (cleanup_receipt>0 AND state IN ('complete','incomplete')))
);
CREATE TABLE workload_artifact_file (
    id BIGSERIAL PRIMARY KEY,
    publication BIGINT NOT NULL,
    artifact_version BIGINT NOT NULL UNIQUE,
    relative_path TEXT NOT NULL,
    max_bytes BIGINT NOT NULL CHECK (max_bytes>0),
    state TEXT NOT NULL DEFAULT 'prepared' CHECK (state IN ('prepared','reserved','published','missing','rejected')),
    UNIQUE(publication,relative_path)
);

-- Hold the same operation lock as Run close, through the allocation commit.
-- An allocator admitted first either commits into the inventory or rolls back;
-- one admitted later cannot insert a version even while execution is Running.
CREATE FUNCTION guard_workload_artifact_file() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE operation JSONB; publication_state TEXT;
BEGIN
    IF TG_OP='UPDATE' THEN
        IF ROW(NEW.id,NEW.publication,NEW.artifact_version,NEW.relative_path,NEW.max_bytes)
           IS DISTINCT FROM ROW(OLD.id,OLD.publication,OLD.artifact_version,OLD.relative_path,OLD.max_bytes)
           OR NOT (NEW.state=OLD.state OR (OLD.state='prepared' AND NEW.state IN ('reserved','missing','rejected'))
               OR (OLD.state='reserved' AND NEW.state IN ('published','rejected')))
        THEN RAISE EXCEPTION 'Prepared artifact binding and terminal state are immutable'; END IF;
        RETURN NEW;
    END IF;
    SELECT l.operation INTO operation FROM workload_launch l
        JOIN workload_artifact_publication p ON p.launch=l.id
        WHERE p.id=NEW.publication FOR UPDATE OF l;
    SELECT state INTO publication_state FROM workload_artifact_publication WHERE id=NEW.publication;
    IF operation IS NULL OR publication_state IS DISTINCT FROM 'open'
        OR operation#>>'{status,state}' IS DISTINCT FROM 'created'
        OR operation#>>'{status,runtime_state}' IS DISTINCT FROM 'running'
        OR NOT COALESCE((operation->>'lease_deadline')::timestamptz>clock_timestamp(),false)
    THEN RAISE EXCEPTION 'Artifact allocation requires an open running launch'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workload_artifact_file_guard BEFORE INSERT OR UPDATE ON workload_artifact_file
FOR EACH ROW EXECUTE FUNCTION guard_workload_artifact_file();

-- The inventory is the immutable set of binding rows, not a later scan of all
-- versions attributed to an execution. This trigger never locks artifact rows.
CREATE FUNCTION close_workload_artifact_inventory() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.operation#>>'{status,state}' IN ('cleanup_required','stop_required','stopped') THEN
        UPDATE workload_artifact_publication SET state='pending',closed_at=clock_timestamp(),
            deadline=clock_timestamp()+interval '300 seconds'
            WHERE launch=NEW.id AND state='open';
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER workload_artifact_inventory_close AFTER UPDATE ON workload_launch
FOR EACH ROW EXECUTE FUNCTION close_workload_artifact_inventory();

CREATE FUNCTION guard_workload_artifact_publication() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.id,NEW.launch,NEW.max_total_bytes,NEW.max_entries) IS DISTINCT FROM ROW(OLD.id,OLD.launch,OLD.max_total_bytes,OLD.max_entries)
        OR (OLD.closed_at IS NOT NULL AND ROW(NEW.closed_at,NEW.deadline) IS DISTINCT FROM ROW(OLD.closed_at,OLD.deadline))
        OR NEW.generation<OLD.generation
        OR NOT (NEW.state=OLD.state OR (OLD.state='open' AND NEW.state='pending')
            OR (OLD.state='pending' AND NEW.state IN ('publishing','complete','incomplete'))
            OR (OLD.state='publishing' AND NEW.state IN ('complete','incomplete')))
        OR (OLD.cleanup_receipt IS NOT NULL AND NEW.cleanup_receipt IS DISTINCT FROM OLD.cleanup_receipt)
    THEN RAISE EXCEPTION 'Artifact publication cannot reopen or erase settlement'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workload_artifact_publication_guard BEFORE UPDATE ON workload_artifact_publication
FOR EACH ROW EXECUTE FUNCTION guard_workload_artifact_publication();
CREATE TRIGGER workload_artifact_publication_retain BEFORE DELETE ON workload_artifact_publication
FOR EACH ROW EXECUTE FUNCTION retain_workload_control_tombstone();
CREATE TRIGGER workload_artifact_file_retain BEFORE DELETE ON workload_artifact_file
FOR EACH ROW EXECUTE FUNCTION retain_workload_control_tombstone();
