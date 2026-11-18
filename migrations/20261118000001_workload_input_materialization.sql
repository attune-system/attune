-- Reserve immutable public input ownership before an external create-only PUT.
-- The row and cancelled state survive source/execution/object retention.
CREATE TABLE workload_input_materialization (
    id BIGSERIAL PRIMARY KEY,
    admission BIGINT NOT NULL,
    kind TEXT NOT NULL CHECK (kind='pack'),
    source_target JSONB NOT NULL,
    source_epochs JSONB NOT NULL,
    release_sha256 TEXT NOT NULL CHECK (release_sha256 ~ '^[0-9a-f]{64}$'),
    object_key TEXT NOT NULL UNIQUE,
    size_bytes BIGINT NOT NULL CHECK (size_bytes>0),
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    expanded_bytes BIGINT NOT NULL CHECK (expanded_bytes>0),
    entries BIGINT NOT NULL CHECK (entries>0),
    state TEXT NOT NULL DEFAULT 'reserved' CHECK (state IN ('reserved','published','cancelled')),
    provider_version TEXT,
    object_id BIGINT,
    cleanup_token UUID NOT NULL DEFAULT gen_random_uuid(),
    cleanup_required BOOLEAN NOT NULL DEFAULT FALSE,
    cleanup_settled BOOLEAN NOT NULL DEFAULT FALSE,
    cleanup_provider_version TEXT,
    UNIQUE(admission,kind),
    CHECK ((provider_version IS NULL)=(object_id IS NULL)),
    CHECK (state<>'published' OR provider_version IS NOT NULL),
    CHECK (NOT cleanup_required OR state='cancelled'),
    CHECK (NOT cleanup_settled OR (cleanup_required AND cleanup_provider_version IS NOT NULL))
);
CREATE FUNCTION guard_workload_input_materialization() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.id,NEW.admission,NEW.kind,NEW.source_target,NEW.source_epochs,NEW.release_sha256,
        NEW.object_key,NEW.size_bytes,NEW.sha256,NEW.expanded_bytes,NEW.entries,NEW.cleanup_token)
       IS DISTINCT FROM ROW(OLD.id,OLD.admission,OLD.kind,OLD.source_target,OLD.source_epochs,OLD.release_sha256,
        OLD.object_key,OLD.size_bytes,OLD.sha256,OLD.expanded_bytes,OLD.entries,OLD.cleanup_token)
       OR NOT (NEW.state=OLD.state OR (OLD.state='reserved' AND NEW.state IN ('published','cancelled'))
           OR (OLD.state='published' AND NEW.state='cancelled'))
       OR (OLD.object_id IS NOT NULL AND ROW(NEW.object_id,NEW.provider_version) IS DISTINCT FROM ROW(OLD.object_id,OLD.provider_version))
       OR (OLD.cleanup_required AND NOT NEW.cleanup_required)
       OR (OLD.cleanup_settled AND ROW(NEW.cleanup_settled,NEW.cleanup_provider_version) IS DISTINCT FROM ROW(OLD.cleanup_settled,OLD.cleanup_provider_version))
    THEN RAISE EXCEPTION 'Input materialization binding and settlement are immutable'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workload_input_materialization_guard BEFORE UPDATE ON workload_input_materialization
FOR EACH ROW EXECUTE FUNCTION guard_workload_input_materialization();
CREATE TRIGGER workload_input_materialization_retain BEFORE DELETE ON workload_input_materialization
FOR EACH ROW EXECUTE FUNCTION retain_workload_control_tombstone();
CREATE TABLE workload_input_maintenance (
    id BIGINT PRIMARY KEY CHECK (id IN (1,2)),
    after_id BIGINT NOT NULL DEFAULT 0,
    high_water BIGINT NOT NULL DEFAULT 0,
    CHECK (after_id>=0 AND high_water>=after_id)
);
INSERT INTO workload_input_maintenance(id) VALUES(1),(2);
