-- Evidence ownership outlives artifact metadata and all physical cleanup waits.
-- Runtime/event IDs remain plain values. No FK keeps a process credential live.
-- This pre-production migration cannot reconstruct already-lost dependencies.
-- Never silently adopt old reservations as if their evidence were protected.
DO $$ BEGIN
    IF EXISTS(SELECT 1 FROM artifact_file_origin) THEN
        RAISE EXCEPTION 'File evidence retention requires an empty reservation set or an explicit audited backfill before this migration';
    END IF;
END $$;
CREATE TABLE file_evidence_owner (
    version BIGINT PRIMARY KEY,
    artifact BIGINT NOT NULL,
    root_read BIGINT NOT NULL,
    reservation JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(reservation::text)<=1048576),
    cleanup_group BIGINT,
    metadata_gone BOOLEAN NOT NULL DEFAULT FALSE,
    object_key TEXT,
    provider_version TEXT,
    object_done BOOLEAN NOT NULL DEFAULT FALSE,
    staging_path TEXT,
    staging_done BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE TABLE file_evidence_pin (
    owner BIGINT NOT NULL REFERENCES file_evidence_owner(version) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    source BIGINT NOT NULL CHECK (source>0),
    source_created TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    PRIMARY KEY(owner,kind,source,source_created)
);
-- Internal storage operations, not credentials. No lease TTL manufactures
-- proof that a possibly still-running PUT has stopped.
CREATE TABLE file_evidence_put (
    attempt UUID PRIMARY KEY,
    owner BIGINT NOT NULL REFERENCES file_evidence_owner(version)
);
CREATE INDEX file_evidence_source ON file_evidence_pin(kind,source,source_created);
CREATE INDEX file_evidence_time ON file_evidence_pin(source_created) WHERE kind='event';
CREATE TRIGGER file_evidence_pin_immutable BEFORE UPDATE ON file_evidence_pin FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();
CREATE FUNCTION guard_file_evidence_owner() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF NOT (OLD.metadata_gone AND OLD.object_done AND OLD.staging_done) THEN RAISE EXCEPTION 'File evidence cleanup is incomplete'; END IF;
        RETURN OLD;
    END IF;
    IF NEW.version<>OLD.version OR NEW.artifact<>OLD.artifact OR NEW.root_read<>OLD.root_read OR NEW.reservation IS DISTINCT FROM OLD.reservation
       OR (OLD.metadata_gone AND NOT NEW.metadata_gone) THEN RAISE EXCEPTION 'File evidence identity is immutable'; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER file_evidence_owner_guard BEFORE UPDATE OR DELETE ON file_evidence_owner FOR EACH ROW EXECUTE FUNCTION guard_file_evidence_owner();
CREATE FUNCTION guard_file_evidence_pin_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM file_evidence_owner WHERE version=OLD.owner) THEN RAISE EXCEPTION 'File evidence pins release only with completed cleanup'; END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER file_evidence_pin_delete BEFORE DELETE ON file_evidence_pin FOR EACH ROW EXECUTE FUNCTION guard_file_evidence_pin_delete();

CREATE FUNCTION file_evidence_graph_held(exclusive_required BOOLEAN) RETURNS BOOLEAN LANGUAGE SQL AS $$
    SELECT EXISTS(SELECT 1 FROM pg_locks
      WHERE locktype='advisory' AND pid=pg_backend_pid() AND granted AND objsubid=1
        AND classid=((hashtextextended(current_schema() || ':named-type-graph',0)>>32)&4294967295)::OID
        AND objid=(hashtextextended(current_schema() || ':named-type-graph',0)&4294967295)::OID
        AND (NOT exclusive_required OR mode='ExclusiveLock'))
$$;
CREATE FUNCTION file_evidence_gc_entry() RETURNS VOID LANGUAGE plpgsql AS $$
BEGIN
    IF file_evidence_graph_held(false) AND NOT file_evidence_graph_held(true) THEN
        RAISE EXCEPTION 'Reclamation must enter exclusive graph gate before source/target locks';
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(current_schema() || ':named-type-graph',0));
END;
$$;
CREATE FUNCTION file_evidence_pinned(k TEXT,s BIGINT,t TIMESTAMPTZ DEFAULT '-infinity') RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT EXISTS(SELECT 1 FROM file_evidence_pin WHERE kind=k AND source=s AND source_created=t)
$$;

-- A cleanup request may contain an entire cycle, but must not absorb any
-- outside dependent. Prune until the remaining requested set is closed.
CREATE FUNCTION file_evidence_request_cleanup(requested BIGINT[]) RETURNS BIGINT[] LANGUAGE plpgsql AS $$
DECLARE allowed BIGINT[]; previous BIGINT[]; group_id BIGINT;
BEGIN
    PERFORM file_evidence_gc_entry();
    IF cardinality(requested)>256 THEN RAISE EXCEPTION 'File cleanup request exceeds bound'; END IF;
    SELECT COALESCE(array_agg(DISTINCT x ORDER BY x),'{}') INTO allowed FROM unnest(requested) x;
    LOOP
        previous:=allowed;
        SELECT COALESCE(array_agg(v ORDER BY v),'{}') INTO allowed FROM unnest(allowed) v
        WHERE NOT EXISTS(SELECT 1 FROM file_evidence_pin p
          WHERE p.kind='artifact_version' AND p.source=v AND p.owner<>v AND NOT p.owner=ANY(allowed));
        EXIT WHEN allowed=previous;
    END LOOP;
    SELECT min(v) INTO group_id FROM unnest(allowed) v;
    -- Existing groups cannot be split or merged while their physical cleanup
    -- is in progress. Repeated single-owner requests are harmless.
    IF EXISTS(SELECT 1 FROM file_evidence_owner o WHERE o.version=ANY(allowed) AND o.cleanup_group IS NOT NULL
       AND EXISTS(SELECT 1 FROM file_evidence_owner peer WHERE peer.cleanup_group=o.cleanup_group AND NOT peer.version=ANY(allowed))) THEN
        RETURN '{}';
    END IF;
    UPDATE file_evidence_owner SET cleanup_group=group_id WHERE version=ANY(allowed);
    RETURN allowed;
END;
$$;
CREATE FUNCTION file_evidence_version_eligible(v BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT NOT EXISTS(SELECT 1 FROM file_evidence_pin p
      LEFT JOIN file_evidence_owner dependent ON dependent.version=p.owner
      LEFT JOIN file_evidence_owner target ON target.version=v
      WHERE p.kind='artifact_version' AND p.source=v AND p.owner<>v
        AND (target.cleanup_group IS NULL OR dependent.cleanup_group IS DISTINCT FROM target.cleanup_group))
$$;
CREATE FUNCTION file_evidence_artifact_eligible(a BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT NOT EXISTS(SELECT 1 FROM file_evidence_pin p JOIN file_evidence_owner o ON o.version=p.owner
      WHERE p.kind='artifact' AND p.source=a AND o.artifact<>a)
      AND NOT EXISTS(SELECT 1 FROM artifact_version v WHERE v.artifact=a AND NOT file_evidence_version_eligible(v.id))
$$;
CREATE FUNCTION file_evidence_release(v BIGINT) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE g BIGINT;
BEGIN
    IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Missing exclusive evidence gate'; END IF;
    SELECT cleanup_group INTO g FROM file_evidence_owner WHERE version=v;
    -- A requested cycle is one cleanup obligation. Releasing one member early
    -- would expose its evidence to GC while another member's body still exists.
    IF g IS NOT NULL THEN
        IF EXISTS(SELECT 1 FROM file_evidence_owner o WHERE cleanup_group=g
          AND (NOT metadata_gone OR NOT object_done OR NOT staging_done
            OR EXISTS(SELECT 1 FROM file_evidence_put p WHERE p.owner=o.version))) THEN RETURN; END IF;
        DELETE FROM file_evidence_owner WHERE cleanup_group=g;
    ELSE
        DELETE FROM file_evidence_owner o WHERE version=v AND metadata_gone AND object_done AND staging_done
          AND NOT EXISTS(SELECT 1 FROM file_evidence_put p WHERE p.owner=o.version);
    END IF;
END;
$$;

CREATE FUNCTION file_evidence_delete_entry() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN PERFORM file_evidence_gc_entry(); RETURN NULL; END;
$$;
CREATE FUNCTION file_evidence_guard_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE source BIGINT; body JSONB:=to_jsonb(OLD);
BEGIN
    IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Missing exclusive evidence gate'; END IF;
    source:=(body->>TG_ARGV[1])::BIGINT;
    IF TG_ARGV[0]='event' THEN
        IF file_evidence_pinned('event',source,(body->>'created')::TIMESTAMPTZ) THEN RETURN NULL; END IF;
    ELSIF TG_ARGV[0]='artifact_version' THEN
        IF NOT file_evidence_version_eligible(source) THEN RETURN NULL; END IF;
    ELSIF TG_ARGV[0]='artifact' THEN
        IF NOT file_evidence_artifact_eligible(source) THEN RETURN NULL; END IF;
    ELSIF file_evidence_pinned(TG_ARGV[0],source) THEN RETURN NULL;
    END IF;
    RETURN OLD;
END;
$$;
DO $$
DECLARE t TEXT;
BEGIN
    FOREACH t IN ARRAY ARRAY['execution','enforcement','artifact','artifact_version','runtime_authority_admission',
      'execution_runtime_read_origin','pack_runtime_policy_revision','pack_release','managed_executable_metadata_revision',
      'component_configuration_revision','sensor_generation_schema_binding','sensor_rule_config_delivery',
       'inquiry','event_origin_snapshot','action','sensor','rule','trigger','pack','work_queue','work_queue_item','schema_type_revision','runtime_external_approval','log_stream'] LOOP
        IF to_regclass(t) IS NOT NULL THEN
            EXECUTE format('CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON %I FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry()',t);
            EXECUTE format('CREATE TRIGGER file_evidence_guard BEFORE DELETE ON %I FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_delete(%L,%L)',t,t,'id');
        END IF;
    END LOOP;
END;
$$;
CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON event FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry();
CREATE TRIGGER file_evidence_guard BEFORE DELETE ON event FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_delete('event','id');

-- Secondary evidence has composite keys. Delete source rows only after their
-- owner was actually deleted, never from a pre-delete list of candidates.
CREATE FUNCTION file_evidence_guard_secondary() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE b JSONB:=to_jsonb(OLD); k TEXT; s BIGINT;
BEGIN
    IF TG_TABLE_NAME='runtime_schema_binding' THEN k:=b->>'entity_kind'; s:=(b->>'entity_id')::BIGINT;
    ELSIF TG_TABLE_NAME='inquiry_schema_binding' THEN k:='inquiry'; s:=(b->>'entity_id')::BIGINT;
    ELSIF TG_TABLE_NAME='event_schema_binding' THEN
        IF EXISTS(SELECT 1 FROM file_evidence_pin WHERE kind='event' AND source=(b->>'event')::BIGINT) THEN RETURN NULL; END IF;
    ELSIF TG_TABLE_NAME='queue_item_schema_binding' THEN k:='work_queue_item'; s:=(b->>'entity_id')::BIGINT;
    ELSIF TG_TABLE_NAME IN ('runtime_authority_release_dependency','runtime_authority_metadata_dependency','runtime_standard_owner_binding','runtime_workflow_policy_dependency') THEN k:='pack_runtime_policy_revision'; s:=(b->>'policy_revision')::BIGINT;
    ELSIF TG_TABLE_NAME='artifact_runtime_owner_binding' THEN k:='artifact'; s:=(b->>'artifact')::BIGINT;
    ELSIF TG_TABLE_NAME='runtime_log_final_manifest' THEN k:='artifact_version'; s:=(b->>'artifact_version')::BIGINT;
    ELSIF TG_TABLE_NAME='log_segment' THEN k:='log_stream'; s:=(b->>'stream')::BIGINT;
    ELSIF TG_TABLE_NAME='pack_release_executable' THEN k:='pack_release'; s:=(b->>'release')::BIGINT;
    ELSIF TG_TABLE_NAME='sensor_rule_structural_origin' THEN
        k:=CASE b->>'source_entity' WHEN 'sensor_generation_rule_config' THEN 'sensor_generation_schema_binding' ELSE 'sensor_rule_config_delivery' END;
        s:=(b->>'source_id')::BIGINT;
    ELSIF TG_TABLE_NAME IN ('execution_runtime_read_fence','execution_key_read_origin','execution_key_read_origin_fence','runtime_authority_execution_attempt','runtime_authority_execution_fence') THEN k:='runtime_authority_admission'; s:=(b->>'admission')::BIGINT;
    ELSIF TG_TABLE_NAME='execution_secret_value' THEN
        k:=CASE b->>'entity_type' WHEN 'execution_config' THEN 'execution' WHEN 'execution_result' THEN 'execution'
          WHEN 'workflow_variables' THEN 'execution' WHEN 'component_configuration' THEN 'component_configuration_revision'
          WHEN 'sensor_generation_config' THEN 'sensor_generation_schema_binding' WHEN 'sensor_generation_rule_config' THEN 'sensor_generation_schema_binding'
          WHEN 'sensor_rule_delivery_config' THEN 'sensor_rule_config_delivery' WHEN 'inquiry_options' THEN 'inquiry' WHEN 'inquiry_response' THEN 'inquiry' WHEN 'queue_item_payload' THEN 'work_queue_item' END;
        s:=(b->>'entity_id')::BIGINT;
    END IF;
    -- Live field replacement is not owner reclamation. C's frozen parents are
    -- authoritative for files, not the replacement's current provenance. Keep
    -- pinned rows when a replace helper deletes before its upsert. Actual owner
    -- deletion/orphan GC must hold EXCLUSIVE, as do all other secondary deletes.
    IF NOT file_evidence_graph_held(true) THEN
        IF TG_TABLE_NAME<>'execution_secret_value' OR NOT file_evidence_graph_held(false) THEN
            RAISE EXCEPTION 'Missing exclusive evidence gate';
        END IF;
    END IF;
    -- Root self-identity is frozen in RuntimeReadOwner. Whole-artifact cleanup
    -- can remove its own binding; outside dependents block the parent DELETE.
    IF TG_TABLE_NAME='artifact_runtime_owner_binding' AND NOT EXISTS(SELECT 1 FROM artifact WHERE id=s) THEN RETURN OLD; END IF;
    IF k IS NOT NULL AND file_evidence_pinned(k,s) THEN RETURN NULL; END IF;
    RETURN OLD;
END;
$$;
DO $$ DECLARE t TEXT; BEGIN
    FOREACH t IN ARRAY ARRAY['runtime_schema_binding','inquiry_schema_binding','event_schema_binding','queue_item_schema_binding','execution_runtime_read_fence','execution_key_read_origin','execution_key_read_origin_fence','runtime_authority_execution_attempt','runtime_authority_execution_fence','execution_secret_value','runtime_authority_release_dependency','runtime_authority_metadata_dependency','runtime_standard_owner_binding','runtime_workflow_policy_dependency','artifact_runtime_owner_binding','runtime_log_final_manifest','log_segment','pack_release_executable','sensor_rule_structural_origin'] LOOP
        IF to_regclass(t) IS NULL THEN CONTINUE; END IF;
        IF t<>'execution_secret_value' THEN
            EXECUTE format('CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON %I FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry()',t);
        END IF;
        EXECUTE format('CREATE TRIGGER file_evidence_guard BEFORE DELETE ON %I FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_secondary()',t);
    END LOOP;
END; $$;
CREATE FUNCTION file_evidence_live_secret_entry() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    -- Ordered live replacement already holds shared graph admission. A standalone
    -- owner-GC statement must enter exclusive, even if this CTE fires first.
    IF NOT file_evidence_graph_held(false) THEN PERFORM file_evidence_gc_entry(); END IF;
    RETURN NULL;
END;
$$;
CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON execution_secret_value FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_live_secret_entry();

CREATE FUNCTION file_evidence_track_body() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        UPDATE file_evidence_owner SET metadata_gone=true WHERE version=OLD.id;
        PERFORM file_evidence_release(OLD.id);
        RETURN OLD;
    END IF;
    UPDATE file_evidence_owner SET object_key=NEW.object_key,provider_version=NEW.provider_version,
      object_done=NEW.object_key IS NULL,staging_path=NEW.file_path,staging_done=NEW.file_path IS NULL WHERE version=NEW.id;
    RETURN NEW;
END;
$$;
CREATE TRIGGER file_evidence_body_update AFTER UPDATE OF object_key,provider_version,file_path ON artifact_version
FOR EACH ROW EXECUTE FUNCTION file_evidence_track_body();
CREATE TRIGGER file_evidence_metadata_delete AFTER DELETE ON artifact_version FOR EACH ROW EXECUTE FUNCTION file_evidence_track_body();

CREATE FUNCTION file_evidence_cleanup_state() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.body_state IS DISTINCT FROM OLD.body_state AND NEW.body_state IN ('deleting','cleanup_claimed')
      AND (EXISTS(SELECT 1 FROM file_evidence_owner WHERE version=OLD.id)
        OR EXISTS(SELECT 1 FROM file_evidence_pin WHERE kind='artifact_version' AND source=OLD.id)) THEN
        IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Cleanup must enter exclusive graph gate before target locks'; END IF;
        IF NOT file_evidence_version_eligible(OLD.id) THEN RETURN NULL; END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER file_evidence_cleanup_state BEFORE UPDATE OF body_state ON artifact_version FOR EACH ROW EXECUTE FUNCTION file_evidence_cleanup_state();

-- A failed final Ready transaction may leave the actual PUT recorded only in
-- the object ledger. Bind that exact provider while preserving its evidence.
CREATE FUNCTION file_evidence_upload_entry() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock_shared(hashtextextended(current_schema() || ':named-type-graph',0));
    RETURN NULL;
END;
$$;
CREATE FUNCTION file_evidence_track_upload() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.provider_version IS NOT NULL THEN
        UPDATE file_evidence_owner SET provider_version=NEW.provider_version,object_done=false
        WHERE object_key=NEW.object_key AND provider_version IS NULL;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER aaa_file_evidence_upload_entry BEFORE INSERT OR UPDATE ON object_maintenance_ledger FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_upload_entry();
CREATE TRIGGER file_evidence_upload_body AFTER INSERT OR UPDATE OF provider_version ON object_maintenance_ledger FOR EACH ROW EXECUTE FUNCTION file_evidence_track_upload();

-- Preserve the existing DDL validation and coverage invalidations, with no
-- alternate callable expiry function that can bypass the pin check.
CREATE OR REPLACE FUNCTION native_partition_expire(managed native_partition_parent,registry_id BIGINT,cutoff TIMESTAMPTZ)
RETURNS BOOLEAN LANGUAGE plpgsql AS $$
DECLARE r native_partition_registry; summary_kind native_summary_kind; summary_name TEXT;
BEGIN
    PERFORM file_evidence_gc_entry();
    EXECUTE format('LOCK TABLE ONLY %I IN ACCESS EXCLUSIVE MODE',managed::text);
    SELECT id,parent,partition_name,lower_bound,upper_bound,created INTO r FROM native_partition_registry WHERE id=registry_id AND parent=managed FOR UPDATE;
    IF NOT FOUND OR r.upper_bound>cutoff THEN RETURN false; END IF;
    IF managed='event' AND EXISTS(SELECT 1 FROM file_evidence_pin WHERE kind='event' AND source_created>=r.lower_bound AND source_created<r.upper_bound) THEN RETURN false; END IF;
    IF r.partition_name<>managed::text || '_p' || to_char(r.lower_bound AT TIME ZONE 'UTC','YYYYMMDD') THEN
        RAISE EXCEPTION 'Incompatible native expiry name: %',r.partition_name USING ERRCODE='55000';
    END IF;
    PERFORM native_partition_check(managed,r.partition_name,r.lower_bound,r.upper_bound);
    FOR summary_kind IN SELECT k FROM unnest(CASE managed
      WHEN 'event' THEN ARRAY['event_volume'::native_summary_kind]
      WHEN 'execution_history' THEN ARRAY['execution_status'::native_summary_kind,'execution_creation'::native_summary_kind]
      ELSE ARRAY[]::native_summary_kind[] END) k ORDER BY k LOOP
        PERFORM 1 FROM native_summary_state WHERE native_summary_state.kind=summary_kind FOR UPDATE;
        summary_name:=CASE summary_kind WHEN 'event_volume' THEN 'event_volume_hourly_summary'
          WHEN 'execution_status' THEN 'execution_status_hourly_summary' ELSE 'execution_creation_hourly_summary' END;
        EXECUTE format('DELETE FROM %I WHERE bucket>=$1 AND bucket<$2',summary_name) USING r.lower_bound,r.upper_bound;
        DELETE FROM native_summary_hour h WHERE h.kind=summary_kind AND h.bucket>=r.lower_bound AND h.bucket<r.upper_bound;
        DELETE FROM native_summary_invalidation n WHERE n.kind=summary_kind AND n.bucket>=r.lower_bound AND n.bucket<r.upper_bound;
    END LOOP;
    EXECUTE format('DROP TABLE %I',r.partition_name);
    DELETE FROM native_partition_registry WHERE id=r.id;
    RETURN true;
END;
$$;
