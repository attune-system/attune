-- Generation-qualified entry identities never overload an event timestamp and
-- never add a reference FK to a native partition. Historical migration bytes stay.
ALTER TABLE file_evidence_pin ADD COLUMN source_generation BIGINT NOT NULL DEFAULT 0 CHECK (source_generation>=0);
ALTER TABLE file_evidence_pin ADD CHECK ((kind='cache_entry')=(source_generation>0));
ALTER TABLE file_evidence_pin DROP CONSTRAINT file_evidence_pin_pkey;
ALTER TABLE file_evidence_pin ADD PRIMARY KEY(owner,kind,source,source_created,source_generation);
CREATE INDEX file_evidence_cache_generation ON file_evidence_pin(source_generation,source) WHERE kind='cache_entry';

CREATE FUNCTION file_evidence_cache_entry_pinned(g BIGINT,e BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT EXISTS(SELECT 1 FROM file_evidence_pin WHERE kind='cache_entry' AND source=e AND source_generation=g)
$$;
CREATE FUNCTION file_evidence_cache_generation_pinned(g BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT file_evidence_pinned('cache_generation',g)
      OR EXISTS(SELECT 1 FROM file_evidence_pin WHERE kind='cache_entry' AND source_generation=g)
$$;

CREATE FUNCTION file_evidence_guard_cache_entry() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF file_evidence_cache_entry_pinned(OLD.generation,OLD.id) THEN
        IF TG_OP='DELETE' THEN RETURN NULL; END IF;
        IF to_jsonb(NEW) IS DISTINCT FROM to_jsonb(OLD) THEN RAISE EXCEPTION 'Pinned native cache entry is immutable'; END IF;
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER file_evidence_native_entry BEFORE DELETE OR UPDATE ON cache_entry
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_cache_entry();

DO $$ DECLARE t TEXT; BEGIN
    FOREACH t IN ARRAY ARRAY['cache_namespace','cache_generation'] LOOP
        EXECUTE format('CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON %I FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry()',t);
        EXECUTE format('CREATE TRIGGER file_evidence_guard BEFORE DELETE ON %I FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_delete(%L,%L)',t,t,'id');
    END LOOP;
END $$;

-- Partition DROP bypasses row triggers. Enter the graph before cache admission,
-- parent protection, target locks or DDL; test exact F pins before ANY DROP.
CREATE OR REPLACE FUNCTION drop_cleanup_cache_generation(generation_id BIGINT, traversal_seconds BIGINT)
RETURNS TABLE(outcome TEXT, records_reclaimed BIGINT, bytes_reclaimed BIGINT)
LANGUAGE plpgsql AS $$
DECLARE
    namespace_id BIGINT;
    namespace_row cache_namespace%ROWTYPE;
    generation_row cache_generation%ROWTYPE;
    usage_row cache_generation_entry_usage%ROWTYPE;
    child_relation TEXT;
BEGIN
    IF traversal_seconds < 0 OR traversal_seconds IS NULL THEN RAISE EXCEPTION 'cache traversal window must be nonnegative'; END IF;
    PERFORM file_evidence_gc_entry();
    IF file_evidence_cache_generation_pinned(generation_id) THEN
        RETURN QUERY SELECT 'ineligible'::TEXT,0::BIGINT,0::BIGINT; RETURN;
    END IF;
    PERFORM pg_advisory_xact_lock(7821101,0);
    LOCK TABLE ONLY cache_entry IN ACCESS EXCLUSIVE MODE;
    SELECT namespace INTO namespace_id FROM cache_generation WHERE id=generation_id;
    IF NOT FOUND THEN RETURN QUERY SELECT 'absent'::TEXT,0::BIGINT,0::BIGINT; RETURN; END IF;
    SELECT * INTO STRICT namespace_row FROM cache_namespace WHERE id=namespace_id FOR UPDATE;
    SELECT * INTO STRICT generation_row FROM cache_generation WHERE id=generation_id FOR UPDATE;
    IF namespace_row.active_generation IS NOT DISTINCT FROM generation_id
       OR NOT COALESCE(generation_row.state='failed' OR (generation_row.state='retired'
          AND generation_row.readable_until<=clock_timestamp()
          AND generation_row.retired<=clock_timestamp()-make_interval(secs=>traversal_seconds::DOUBLE PRECISION)),FALSE)
       OR file_evidence_cache_generation_pinned(generation_id)
       OR EXISTS(SELECT 1 FROM workflow_cache_iteration i JOIN workflow_execution w ON w.id=i.workflow_execution
          WHERE i.generation=generation_id AND i.state='scanning'
          AND w.status NOT IN ('completed','failed','cancelled','timeout','abandoned')) THEN
        RETURN QUERY SELECT 'ineligible'::TEXT,0::BIGINT,0::BIGINT; RETURN;
    END IF;
    child_relation:=validate_cache_generation_partition(generation_id);
    PERFORM 1 FROM cache_deployment_physical_byte_usage WHERE id=1 FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache deployment usage is missing'; END IF;
    PERFORM 1 FROM cache_owner_physical_byte_usage WHERE owner_type=namespace_row.owner_type AND owner=namespace_row.owner FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache owner usage is missing'; END IF;
    SELECT * INTO STRICT usage_row FROM cache_generation_entry_usage WHERE generation=generation_id FOR UPDATE;
    EXECUTE format('DROP TABLE %s',child_relation);
    UPDATE cache_deployment_physical_byte_usage SET physical_bytes=physical_bytes-usage_row.physical_bytes
      WHERE id=1 AND physical_bytes>=usage_row.physical_bytes;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache deployment usage underflow'; END IF;
    UPDATE cache_owner_physical_byte_usage SET physical_bytes=physical_bytes-usage_row.physical_bytes
      WHERE owner_type=namespace_row.owner_type AND owner=namespace_row.owner AND physical_bytes>=usage_row.physical_bytes;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache owner usage underflow'; END IF;
    DELETE FROM cache_generation_entry_usage WHERE generation=generation_id;
    DELETE FROM cache_ingest_chunk WHERE generation=generation_id;
    DELETE FROM cache_generation WHERE id=generation_id;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache generation disappeared during reclamation'; END IF;
    UPDATE cache_entry_statistics_state SET partitions_dropped=partitions_dropped+1 WHERE id=TRUE;
    IF NOT FOUND THEN RAISE EXCEPTION 'cache statistics state is missing'; END IF;
    RETURN QUERY SELECT 'dropped'::TEXT,usage_row.record_count,usage_row.physical_bytes;
END; $$;

-- 06012's final raw-admission delete override is intentionally NOT replaced.
-- Its budget guard now also recognizes the explicit typed budget owner pin.
CREATE OR REPLACE FUNCTION file_evidence_guard_native_archive() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE pinned BOOLEAN;
BEGIN
    IF TG_TABLE_NAME='execution_secret_value' THEN
        IF OLD.entity_type<>'native_input_snapshot' THEN
            IF TG_OP='UPDATE' THEN RETURN NEW; END IF; RETURN OLD;
        END IF;
        pinned:=file_evidence_pinned('runtime_native_input_snapshot',OLD.entity_id);
    ELSE
        pinned:=file_evidence_pinned('runtime_native_input_snapshot_budget',OLD.renderer_admission)
          OR file_evidence_pinned('runtime_authority_admission',OLD.renderer_admission)
          OR EXISTS(SELECT 1 FROM runtime_native_input_snapshot s WHERE s.renderer_admission=OLD.renderer_admission
             AND file_evidence_pinned('runtime_native_input_snapshot',s.id));
    END IF;
    IF TG_OP='UPDATE' THEN
        IF pinned THEN RAISE EXCEPTION 'Pinned native snapshot ciphertext/budget is immutable'; END IF; RETURN NEW;
    END IF;
    IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Native archive GC requires exclusive graph admission'; END IF;
    IF pinned THEN RETURN NULL; END IF; RETURN OLD;
END; $$;
