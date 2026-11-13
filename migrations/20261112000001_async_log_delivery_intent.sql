-- Bounded custody handoffs for delegated object logs. Runtime enablement is
-- coordinated with worker, API, reader and client changes; no historical grant.
CREATE TABLE log_delivery_intent (
    id BIGSERIAL PRIMARY KEY,
    execution BIGINT NOT NULL,
    artifact_version BIGINT NOT NULL UNIQUE REFERENCES artifact_version(id) ON DELETE CASCADE,
    stream BIGINT NOT NULL UNIQUE REFERENCES log_stream(id) ON DELETE CASCADE,
    worker BIGINT NOT NULL,
    source_instance UUID NOT NULL,
    delivery_instance UUID NOT NULL,
    attempt UUID NOT NULL,
    owner BIGINT NOT NULL,
    custody UUID NOT NULL UNIQUE,
    state TEXT NOT NULL DEFAULT 'prepared' CHECK (state IN ('prepared','active','ready','incomplete','revoked')),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    segments JSONB NOT NULL CHECK (jsonb_typeof(segments)='array' AND jsonb_array_length(segments)>0),
    total_bytes BIGINT NOT NULL CHECK (total_bytes > 0),
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    truncated BOOLEAN NOT NULL,
    authenticated_manifest TEXT,
    finalize_by TIMESTAMPTZ NOT NULL,
    failure TEXT CHECK (failure IN ('deadline','custody_lost','delivery_failed','authority_revoked')),
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (execution>0 AND worker>0 AND owner>0),
    CHECK (finalize_by>created),
    CHECK (source_instance <> '00000000-0000-0000-0000-000000000000'::uuid),
    CHECK (delivery_instance <> '00000000-0000-0000-0000-000000000000'::uuid),
    CHECK (attempt <> '00000000-0000-0000-0000-000000000000'::uuid),
    CHECK (custody <> '00000000-0000-0000-0000-000000000000'::uuid)
);
CREATE INDEX log_delivery_due ON log_delivery_intent(finalize_by,id) WHERE state IN ('prepared','active');
CREATE INDEX log_delivery_execution ON log_delivery_intent(execution);
CREATE INDEX log_delivery_owner ON log_delivery_intent(owner) WHERE state IN ('prepared','active');

CREATE FUNCTION guard_log_delivery_intent() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.id,NEW.execution,NEW.artifact_version,NEW.stream,NEW.worker,NEW.source_instance,
           NEW.attempt,NEW.owner,NEW.custody,NEW.segments,NEW.total_bytes,NEW.sha256,
           NEW.truncated,NEW.authenticated_manifest,NEW.finalize_by,NEW.created)
       IS DISTINCT FROM
       ROW(OLD.id,OLD.execution,OLD.artifact_version,OLD.stream,OLD.worker,OLD.source_instance,
           OLD.attempt,OLD.owner,OLD.custody,OLD.segments,OLD.total_bytes,OLD.sha256,
           OLD.truncated,OLD.authenticated_manifest,OLD.finalize_by,OLD.created) THEN
        RAISE EXCEPTION 'Log delivery handoff is immutable';
    END IF;
    IF OLD.state IN ('ready','incomplete','revoked') AND ROW(NEW.state,NEW.delivery_instance,NEW.failure) IS DISTINCT FROM ROW(OLD.state,OLD.delivery_instance,OLD.failure) THEN
        RAISE EXCEPTION 'Log delivery terminal state is immutable';
    END IF;
    IF NEW.state='ready' AND NOT EXISTS (
        SELECT 1 FROM artifact_version v JOIN log_stream s ON s.artifact_version=v.id
        WHERE v.id=NEW.artifact_version AND s.id=NEW.stream AND v.body_state='ready'
          AND s.sealed AND s.close_phase='published' AND s.total_bytes=NEW.total_bytes
          AND v.sha256=NEW.sha256 AND s.truncated=NEW.truncated
    ) THEN RAISE EXCEPTION 'Log delivery requires verified Ready publication'; END IF;
    NEW.updated=clock_timestamp();
    NEW.revision=OLD.revision+1;
    RETURN NEW;
END $$;
CREATE TRIGGER log_delivery_immutable BEFORE UPDATE ON log_delivery_intent FOR EACH ROW EXECUTE FUNCTION guard_log_delivery_intent();

-- Source changes cancel pending custody permanently. They do not delete the
-- execution, resurrect a capability, or reveal any log content.
CREATE FUNCTION revoke_log_delivery_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(OLD.frozen,OLD.attributes) IS DISTINCT FROM ROW(NEW.frozen,NEW.attributes) THEN
        UPDATE log_delivery_intent SET state='revoked',failure='authority_revoked'
        WHERE owner=OLD.id AND state IN ('prepared','active');
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_delivery_identity_revocation AFTER UPDATE OR DELETE ON identity FOR EACH ROW EXECUTE FUNCTION revoke_log_delivery_identity();
CREATE FUNCTION revoke_log_delivery_assignment() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE log_delivery_intent SET state='revoked',failure='authority_revoked'
    WHERE owner=OLD.identity AND state IN ('prepared','active');
    RETURN NULL;
END $$;
CREATE TRIGGER log_delivery_permission_assignment_revocation AFTER UPDATE OR DELETE ON permission_assignment FOR EACH ROW EXECUTE FUNCTION revoke_log_delivery_assignment();
CREATE TRIGGER log_delivery_role_assignment_revocation AFTER UPDATE OR DELETE ON identity_role_assignment FOR EACH ROW EXECUTE FUNCTION revoke_log_delivery_assignment();
CREATE FUNCTION revoke_log_delivery_grants() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='permission_set' THEN
        UPDATE log_delivery_intent i SET state='revoked',failure='authority_revoked'
        WHERE i.state IN ('prepared','active') AND (
            EXISTS(SELECT 1 FROM execution_authority_selection s WHERE s.execution=i.execution AND OLD.ref=ANY(s.selected_refs)) OR
            EXISTS(SELECT 1 FROM permission_assignment a WHERE a.identity=i.owner AND a.permset=OLD.id) OR
            EXISTS(SELECT 1 FROM identity_role_assignment a JOIN permission_set_role_assignment r ON r.role=a.role WHERE a.identity=i.owner AND r.permset=OLD.id)
        );
    ELSE
        UPDATE log_delivery_intent i SET state='revoked',failure='authority_revoked'
        WHERE i.state IN ('prepared','active') AND EXISTS(SELECT 1 FROM identity_role_assignment a WHERE a.identity=i.owner AND a.role=OLD.role);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_delivery_permission_definition_revocation AFTER UPDATE OR DELETE ON permission_set FOR EACH ROW EXECUTE FUNCTION revoke_log_delivery_grants();
CREATE TRIGGER log_delivery_role_definition_revocation AFTER UPDATE OR DELETE ON permission_set_role_assignment FOR EACH ROW EXECUTE FUNCTION revoke_log_delivery_grants();
CREATE FUNCTION revoke_log_delivery_execution() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(OLD.worker,OLD.executor,OLD.action,OLD.executable_snapshot,OLD.permission_set_refs) IS DISTINCT FROM ROW(NEW.worker,NEW.executor,NEW.action,NEW.executable_snapshot,NEW.permission_set_refs) THEN
        UPDATE log_delivery_intent SET state='revoked',failure='authority_revoked'
        WHERE execution=OLD.id AND state IN ('prepared','active');
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_delivery_execution_revocation AFTER UPDATE OR DELETE ON execution FOR EACH ROW EXECUTE FUNCTION revoke_log_delivery_execution();
CREATE FUNCTION activate_log_delivery_on_terminal() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status IN ('running','canceling') AND NEW.status IN ('completed','failed','cancelled','timeout','abandoned') THEN
        UPDATE log_delivery_intent SET state=CASE WHEN finalize_by>clock_timestamp() THEN 'active' ELSE 'incomplete' END,
           failure=CASE WHEN finalize_by>clock_timestamp() THEN NULL ELSE 'deadline' END
        WHERE execution=NEW.id AND worker=NEW.worker AND state='prepared';
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_delivery_execution_terminal AFTER UPDATE ON execution FOR EACH ROW EXECUTE FUNCTION activate_log_delivery_on_terminal();

CREATE FUNCTION log_delivery_pinned(v BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM log_delivery_intent WHERE artifact_version=v AND state IN ('prepared','active') AND finalize_by>clock_timestamp())
$$;
ALTER FUNCTION file_evidence_version_eligible(BIGINT) RENAME TO file_evidence_version_eligible_without_log_delivery;
CREATE FUNCTION file_evidence_version_eligible(v BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
 SELECT NOT log_delivery_pinned(v) AND file_evidence_version_eligible_without_log_delivery(v)
$$;
ALTER FUNCTION file_evidence_request_cleanup(BIGINT[]) RENAME TO file_evidence_request_cleanup_without_log_delivery;
CREATE FUNCTION file_evidence_request_cleanup(requested BIGINT[]) RETURNS BIGINT[] LANGUAGE plpgsql AS $$
BEGIN
 PERFORM file_evidence_gc_entry();
 IF EXISTS(SELECT 1 FROM unnest(requested) v WHERE log_delivery_pinned(v)) THEN RETURN '{}'; END IF;
 RETURN file_evidence_request_cleanup_without_log_delivery(requested);
END $$;
CREATE FUNCTION guard_log_delivery_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE v BIGINT;
BEGIN
 IF TG_TABLE_NAME='artifact_version' THEN v=OLD.id; ELSE v=OLD.artifact_version; END IF;
 IF log_delivery_pinned(v) THEN RAISE EXCEPTION 'Active log delivery retains its version and stream'; END IF;
 RETURN OLD;
END $$;
CREATE TRIGGER aab_log_delivery_version BEFORE DELETE ON artifact_version FOR EACH ROW EXECUTE FUNCTION guard_log_delivery_delete();
CREATE TRIGGER aab_log_delivery_stream BEFORE DELETE ON log_stream FOR EACH ROW EXECUTE FUNCTION guard_log_delivery_delete();
-- Attempt replacement revokes the old custody permanently; merely closing the
-- action API at completion is expected and leaves only exact delivery authority.
CREATE FUNCTION revoke_log_delivery_attempt() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR ROW(OLD.attempt,OLD.worker,OLD.producer_kind) IS DISTINCT FROM ROW(NEW.attempt,NEW.worker,NEW.producer_kind) THEN
  UPDATE log_delivery_intent SET state='revoked',failure='authority_revoked' WHERE execution=OLD.execution AND state IN ('prepared','active');
 END IF;
 RETURN NULL;
END $$;
CREATE TRIGGER log_delivery_attempt_revocation AFTER UPDATE OR DELETE ON delegated_execution_attempt FOR EACH ROW EXECUTE FUNCTION revoke_log_delivery_attempt();

CREATE FUNCTION revoke_log_delivery_schema_binding() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF OLD.entity_kind='execution' THEN UPDATE log_delivery_intent SET state='revoked',failure='authority_revoked' WHERE execution=OLD.entity_id AND state IN ('prepared','active'); END IF;
 RETURN NULL;
END $$;
CREATE TRIGGER log_delivery_binding_revocation AFTER DELETE ON runtime_schema_binding FOR EACH ROW EXECUTE FUNCTION revoke_log_delivery_schema_binding();

CREATE FUNCTION revoke_log_delivery_artifact_owner() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR NEW IS DISTINCT FROM OLD THEN
  UPDATE log_delivery_intent i SET state='revoked',failure='authority_revoked' WHERE i.state IN ('prepared','active') AND EXISTS(SELECT 1 FROM artifact_version v WHERE v.id=i.artifact_version AND v.artifact=OLD.artifact);
 END IF;
 RETURN NULL;
END $$;
CREATE TRIGGER log_delivery_artifact_owner_revocation AFTER UPDATE OR DELETE ON artifact_runtime_owner_binding FOR EACH ROW EXECUTE FUNCTION revoke_log_delivery_artifact_owner();

-- Only incarnation replacement or loss of eligibility conflicts with delivery.
-- A worker row SHARE lock would starve heartbeat updates under overlapping HTTP
-- requests; this dedicated fence leaves ordinary heartbeat/status writes free.
CREATE FUNCTION fence_log_delivery_worker() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR
       (OLD.capabilities->>'log_delivery_instance') IS DISTINCT FROM (NEW.capabilities->>'log_delivery_instance') OR
       (OLD.status IN ('active','busy')) IS DISTINCT FROM (NEW.status IN ('active','busy')) THEN
        PERFORM pg_advisory_xact_lock(hashtextextended(current_schema() || ':log-delivery-worker:' || OLD.id::text, 0));
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER log_delivery_worker_fence BEFORE UPDATE OR DELETE ON worker FOR EACH ROW EXECUTE FUNCTION fence_log_delivery_worker();
