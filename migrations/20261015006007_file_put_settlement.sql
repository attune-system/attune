-- Protocol tags are storage operations, never runtime credentials. Old native
-- multipart/copy operations have no durable staging identity and cannot be
-- declared settled from age, process cancellation or HEAD absence.
ALTER TABLE file_evidence_put
    ADD COLUMN protocol TEXT NOT NULL DEFAULT 'legacy_unknown'
        CHECK (protocol IN ('legacy_unknown','atomic_create_v1')),
    ADD COLUMN execution BIGINT,
    ADD COLUMN execution_attempt UUID,
    ADD COLUMN worker_instance UUID,
    ADD COLUMN native_instance UUID,
    ADD COLUMN diagnostic TEXT;
CREATE TRIGGER file_evidence_put_immutable BEFORE UPDATE ON file_evidence_put
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();
CREATE TABLE file_evidence_put_observation (
    attempt UUID PRIMARY KEY REFERENCES file_evidence_put(attempt) ON DELETE CASCADE,
    object_key TEXT NOT NULL,
    provider_version TEXT NOT NULL
);
CREATE TRIGGER file_evidence_put_observation_immutable BEFORE UPDATE ON file_evidence_put_observation
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();
ALTER TABLE object_maintenance_ledger DROP CONSTRAINT object_maintenance_ledger_state_check;
ALTER TABLE object_maintenance_ledger ADD CONSTRAINT object_maintenance_ledger_state_check
    CHECK (state IN ('uploading','ready','deletion_pending','deleting','write_fencing','write_fenced'));
ALTER TABLE object_maintenance_ledger DROP CONSTRAINT object_maintenance_ledger_check;
ALTER TABLE object_maintenance_ledger ADD CONSTRAINT object_maintenance_ledger_check
    CHECK ((state IN ('uploading','write_fencing') AND provider_version IS NULL)
        OR (state<>'uploading' AND provider_version IS NOT NULL));
ALTER TABLE file_evidence_orphan_object ADD COLUMN transport_unresolved BOOLEAN NOT NULL DEFAULT true;
-- Unknown-origin legacy object operations also need an audit of their native
-- writer/staging lifetime before ordinary provider collection can proceed.
UPDATE object_maintenance_ledger l SET state='write_fencing'
WHERE EXISTS(SELECT 1 FROM file_evidence_orphan_object o WHERE o.object_key=l.object_key);

CREATE TABLE file_evidence_write_fence (
    object_key TEXT PRIMARY KEY,
    token UUID NOT NULL UNIQUE,
    version BIGINT NOT NULL,
    provider_version TEXT,
    confirmed BOOLEAN NOT NULL DEFAULT false
);
-- These tiny, non-content provider sentinels permanently reserve abandoned keys.
-- Removing one would permit an old conditional PUT to recreate the file body.
CREATE FUNCTION file_evidence_guard_write_fence() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN RAISE EXCEPTION 'A permanent file write fence cannot be collected'; END IF;
    IF NEW.object_key<>OLD.object_key OR NEW.token<>OLD.token OR NEW.version<>OLD.version
       OR (OLD.confirmed AND NOT NEW.confirmed)
       OR (OLD.provider_version IS NOT NULL AND NEW.provider_version IS DISTINCT FROM OLD.provider_version) THEN
        RAISE EXCEPTION 'File write fence identity is immutable';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER file_evidence_write_fence_immutable BEFORE UPDATE OR DELETE ON file_evidence_write_fence
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_write_fence();

-- A legacy reservation may have a remote writer whose random staging identity
-- was never recorded. Preserve that unknown obligation explicitly. It needs a
-- provider/native writer audit, not a synthetic settlement receipt.
INSERT INTO file_evidence_put(attempt,owner,protocol)
SELECT gen_random_uuid(),o.version,'legacy_unknown' FROM file_evidence_owner o
WHERE o.object_key IS NOT NULL AND NOT o.object_done
    AND NOT EXISTS(SELECT 1 FROM file_evidence_put p WHERE p.owner=o.version);

-- Existing rows cannot be updated through their immutable trigger. This
-- dedicated diagnostic view locates the missing native protocol instead.
CREATE VIEW file_evidence_put_diagnostic AS
SELECT p.attempt,p.owner,o.object_key,p.execution,p.execution_attempt,p.worker_instance,p.native_instance,p.protocol,
    CASE WHEN p.protocol='legacy_unknown' THEN
        'Legacy native writer/staging identity missing; stop and audit the original writer and provider multipart staging; HEAD absence and TTL cannot settle this operation'
    ELSE COALESCE(p.diagnostic,'Atomic native operation unresolved; exact-provider key fencing required after metadata cleanup') END AS diagnostic
FROM file_evidence_put p JOIN file_evidence_owner o ON o.version=p.owner;

CREATE FUNCTION file_evidence_guard_fenced_ledger() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM file_evidence_write_fence f WHERE f.object_key=OLD.object_key) THEN
        -- No ordinary upload/retention path can resurrect or collect a sentinel.
        -- Settlement updates its own fence row, never the old payload ledger.
        IF TG_OP='UPDATE' AND NEW.state='write_fenced' AND file_evidence_graph_held(true)
           AND EXISTS(SELECT 1 FROM file_evidence_write_fence f WHERE f.object_key=OLD.object_key
             AND f.confirmed AND f.provider_version=NEW.provider_version) THEN RETURN NEW; END IF;
        RETURN NULL;
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER file_evidence_fenced_ledger BEFORE UPDATE OR DELETE ON object_maintenance_ledger
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_fenced_ledger();

CREATE FUNCTION file_evidence_guard_put_retirement() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT file_evidence_graph_held(false) THEN RAISE EXCEPTION 'PUT retirement must enter the graph gate before source/target locks'; END IF;
    IF EXISTS(SELECT 1 FROM file_evidence_put_observation r JOIN file_evidence_owner o ON o.version=OLD.owner
        WHERE r.attempt=OLD.attempt AND r.object_key=o.object_key AND r.provider_version=o.provider_version) THEN RETURN OLD; END IF;
    IF OLD.protocol='atomic_create_v1' AND EXISTS(SELECT 1 FROM file_evidence_write_fence f
        WHERE f.version=OLD.owner AND f.confirmed) AND file_evidence_graph_held(true) THEN RETURN OLD; END IF;
    RAISE EXCEPTION 'Native PUT settlement receipt missing for operation % owner %; cancellation, age and HEAD absence are not proof',OLD.attempt,OLD.owner;
END;
$$;
CREATE TRIGGER file_evidence_put_retirement BEFORE DELETE ON file_evidence_put
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_put_retirement();
