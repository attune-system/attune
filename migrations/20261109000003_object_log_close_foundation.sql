-- Read-only object-close foundation. The production route does not select closes yet.
-- Selection permanently closes admission; only `closing` supplies the temporary pin.
ALTER TABLE log_stream
    ADD COLUMN close_revision BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN close_phase TEXT,
    ADD COLUMN close_input JSONB,
    ADD COLUMN close_input_sha256 TEXT,
    ADD COLUMN close_deadline TIMESTAMPTZ,
    ADD CONSTRAINT log_close_shape CHECK (
        (close_phase IS NULL AND close_revision=0 AND close_input IS NULL
         AND close_input_sha256 IS NULL AND close_deadline IS NULL)
        OR (close_phase IS NOT NULL AND close_input_sha256 IS NOT NULL AND close_deadline IS NOT NULL AND close_phase IN ('closing','abandoned','published') AND close_revision>0
         AND close_input IS NOT NULL AND jsonb_typeof(close_input)='object'
         AND close_input_sha256 ~ '^[0-9a-f]{64}$' AND close_deadline IS NOT NULL
         AND backend='object_segments'));

CREATE FUNCTION object_log_close_pinned(v BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=v AND close_phase='closing')
$$;
CREATE FUNCTION object_log_close_object_pinned(k TEXT, p TEXT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT EXISTS(SELECT 1 FROM log_segment g JOIN log_stream s ON s.id=g.stream
                  WHERE g.object_key=k AND g.provider_version=p AND s.close_phase='closing')
$$;

-- No graph acquisition behind target locks. Existing statement admission and
-- repository source -> artifact -> version -> stream order remain authoritative.
CREATE FUNCTION guard_object_log_segment() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE s log_stream;
BEGIN
    IF TG_OP='UPDATE' THEN
        IF NEW IS DISTINCT FROM OLD THEN RAISE EXCEPTION 'Accepted log segment identity is immutable'; END IF;
        RETURN NEW;
    END IF;
    IF TG_OP='DELETE' THEN
        IF EXISTS(SELECT 1 FROM log_stream WHERE id=OLD.stream AND close_phase='closing') THEN
            RAISE EXCEPTION 'Object log close pins accepted segments';
        END IF;
        RETURN OLD;
    END IF;
    SELECT * INTO STRICT s FROM log_stream WHERE id=NEW.stream FOR UPDATE;
    IF s.backend<>'object_segments' OR s.sealed OR s.close_phase IS NOT NULL
       OR NEW.sequence<>s.next_sequence OR NEW.byte_start<>s.total_bytes
       OR NOT EXISTS(SELECT 1 FROM artifact_version WHERE id=s.artifact_version AND body_state='pending') THEN
        RAISE EXCEPTION 'Log segment admission is closed or out of order';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER object_log_segment_guard BEFORE INSERT OR UPDATE OR DELETE ON log_segment
    FOR EACH ROW EXECUTE FUNCTION guard_object_log_segment();

-- Canonical internal v1 projection: JSONB text is the sole hash encoding.
-- Include every stored segment field, in sequence order, including creation identity.
CREATE FUNCTION object_log_close_snapshot(sid BIGINT) RETURNS JSONB LANGUAGE SQL STABLE AS $$
    SELECT jsonb_build_object('v',1,'artifact',v.artifact,'version',v.id,'stream',s.id,
        'backend',s.backend,'next_sequence',s.next_sequence,'total_bytes',s.total_bytes,
        'segments',COALESCE((SELECT jsonb_agg(to_jsonb(g) ORDER BY g.sequence)
                            FROM log_segment g WHERE g.stream=s.id),'[]'::jsonb))
    FROM log_stream s JOIN artifact_version v ON v.id=s.artifact_version WHERE s.id=sid
$$;
CREATE FUNCTION object_log_close_check_selection(sid BIGINT, input JSONB) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE s log_stream; n BIGINT; bytes BIGINT;
BEGIN
    IF NOT file_evidence_graph_held(false) THEN RAISE EXCEPTION 'Missing evidence admission'; END IF;
    SELECT * INTO STRICT s FROM log_stream WHERE id=sid;
    IF s.backend<>'object_segments' OR s.sealed
       OR input->'snapshot' IS DISTINCT FROM object_log_close_snapshot(sid)
       OR jsonb_typeof(input->'producer') IS DISTINCT FROM 'object'
       OR jsonb_typeof(input->'proof') IS DISTINCT FROM 'object'
       OR jsonb_typeof(input->'truncated') IS DISTINCT FROM 'boolean'
       OR NOT EXISTS(SELECT 1 FROM artifact_version WHERE id=s.artifact_version AND body_state='pending')
       OR EXISTS(SELECT 1 FROM file_evidence_owner WHERE version=s.artifact_version AND cleanup_group IS NOT NULL)
       OR EXISTS(SELECT 1 FROM artifact_upload_grant WHERE artifact_version=s.artifact_version
                 AND segment_sequence IS NOT NULL AND state='issued' AND settle_until>clock_timestamp()) THEN
        RAISE EXCEPTION 'Invalid object close selection';
    END IF;
    SELECT count(*),COALESCE(sum(size_bytes),0) INTO n,bytes FROM log_segment WHERE stream=sid;
    IF n<>s.next_sequence OR bytes<>s.total_bytes OR EXISTS(
        SELECT 1 FROM (SELECT sequence,byte_start,lag(byte_end,1,0::bigint) OVER (ORDER BY sequence) previous,
                        row_number() OVER (ORDER BY sequence)-1 expected FROM log_segment WHERE stream=sid) g
        WHERE sequence<>expected OR byte_start<>previous) THEN RAISE EXCEPTION 'Invalid close frontier'; END IF;
    IF EXISTS(SELECT 1 FROM log_segment g WHERE g.stream=sid AND NOT EXISTS(
        SELECT 1 FROM object_maintenance_ledger l WHERE l.object_key=g.object_key
          AND l.provider_version=g.provider_version AND l.state='ready' AND l.size_bytes=g.size_bytes)) THEN
        RAISE EXCEPTION 'Accepted provider object is not ledger-ready';
    END IF;
END $$;

CREATE FUNCTION guard_object_log_close_stream() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='INSERT' THEN
        IF NEW.close_phase IS NOT NULL OR NEW.close_revision<>0 THEN
            RAISE EXCEPTION 'Close selection requires an existing stream';
        END IF;
        RETURN NEW;
    END IF;
    IF TG_OP='DELETE' THEN
        IF OLD.close_phase='closing' OR (OLD.close_phase IS NOT NULL AND
            EXISTS(SELECT 1 FROM artifact_version WHERE id=OLD.artifact_version)) THEN
            RAISE EXCEPTION 'Closed stream survives until allowed version teardown';
        END IF;
        RETURN OLD;
    END IF;
    IF OLD.close_phase IS NOT NULL THEN
        IF ROW(NEW.id,NEW.artifact_version,NEW.backend,NEW.next_sequence,NEW.total_bytes,
               NEW.close_input,NEW.close_input_sha256,NEW.created,
               NEW.max_unflushed_bytes,NEW.max_unflushed_milliseconds)
           IS DISTINCT FROM
           ROW(OLD.id,OLD.artifact_version,OLD.backend,OLD.next_sequence,OLD.total_bytes,
               OLD.close_input,OLD.close_input_sha256,OLD.created,
               OLD.max_unflushed_bytes,OLD.max_unflushed_milliseconds) THEN
            RAISE EXCEPTION 'Selected close identity is permanent';
        END IF;
        IF NEW.close_phase IS NULL OR NEW.close_revision<OLD.close_revision THEN
            RAISE EXCEPTION 'Close cannot reopen or lose revision history';
        END IF;
        IF ROW(NEW.close_phase,NEW.close_revision,NEW.close_deadline) IS DISTINCT FROM
           ROW(OLD.close_phase,OLD.close_revision,OLD.close_deadline) THEN
            IF OLD.close_phase='closing' AND NEW.close_phase='abandoned'
               AND NEW.close_revision=OLD.close_revision+1 AND NEW.close_deadline=OLD.close_deadline THEN
                NULL; -- explicit abandonment invalidates every old verifier
            ELSIF NEW.close_phase='closing' AND NEW.close_revision=OLD.close_revision+1
               AND (OLD.close_phase='abandoned' OR (OLD.close_phase='closing' AND OLD.close_deadline<=clock_timestamp()))
               AND NEW.close_deadline>clock_timestamp() THEN
                PERFORM object_log_close_check_selection(OLD.id,NEW.close_input);
            ELSE
                RAISE EXCEPTION 'Invalid close revision or phase transition';
            END IF;
        END IF;
    ELSIF NEW.close_phase IS NOT NULL THEN
        IF NEW.close_phase<>'closing' OR NEW.close_revision<>1 OR NEW.close_deadline<=clock_timestamp()
           OR ROW(NEW.id,NEW.artifact_version,NEW.backend,NEW.next_sequence,NEW.total_bytes,
                  NEW.sealed,NEW.sealed_at,NEW.truncated) IS DISTINCT FROM
              ROW(OLD.id,OLD.artifact_version,OLD.backend,OLD.next_sequence,OLD.total_bytes,
                  OLD.sealed,OLD.sealed_at,OLD.truncated) THEN
            RAISE EXCEPTION 'Invalid initial close transition';
        END IF;
        PERFORM object_log_close_check_selection(OLD.id,NEW.close_input);
    END IF;
    IF NEW.close_phase IS NOT NULL AND NEW.close_input_sha256 IS DISTINCT FROM
       encode(sha256(convert_to(NEW.close_input::text,'UTF8')),'hex') THEN
        RAISE EXCEPTION 'Invalid canonical close digest';
    END IF;
    IF NEW.close_phase IS NOT NULL AND ROW(NEW.sealed,NEW.sealed_at,NEW.truncated)
       IS DISTINCT FROM ROW(OLD.sealed,OLD.sealed_at,OLD.truncated) THEN
        RAISE EXCEPTION 'Selected close requires verified finalization';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER object_log_close_stream_guard BEFORE INSERT OR UPDATE OR DELETE ON log_stream
    FOR EACH ROW EXECUTE FUNCTION guard_object_log_close_stream();

CREATE FUNCTION guard_object_log_close_version() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF object_log_close_pinned(OLD.id) THEN RAISE EXCEPTION 'Object log close pins version'; END IF;
        RETURN OLD;
    END IF;
    IF EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
       AND (to_jsonb(NEW)-ARRAY['body_state','body_updated','legacy_snapshot_expires_at'])
           IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['body_state','body_updated','legacy_snapshot_expires_at']) THEN
        RAISE EXCEPTION 'Selected close forbids generic body mutation';
    END IF;
    IF NEW.body_state IS DISTINCT FROM OLD.body_state AND EXISTS(
        SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL
          AND (close_phase='closing' OR NEW.body_state NOT IN ('cleanup_claimed','deleting'))) THEN
        RAISE EXCEPTION 'Selected close forbids generic state publication';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER aab_object_log_close_version BEFORE UPDATE OR DELETE ON artifact_version
    FOR EACH ROW EXECUTE FUNCTION guard_object_log_close_version();
CREATE FUNCTION guard_object_log_close_parent() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM artifact_version WHERE artifact=OLD.id AND object_log_close_pinned(id)) THEN
        RAISE EXCEPTION 'Object log close pins artifact';
    END IF;
    RETURN OLD;
END $$;
CREATE TRIGGER aab_object_log_close_parent BEFORE DELETE ON artifact
    FOR EACH ROW EXECUTE FUNCTION guard_object_log_close_parent();

-- Retain the current evidence predicate, including cleanup-group semantics.
ALTER FUNCTION file_evidence_version_eligible(BIGINT) RENAME TO file_evidence_version_eligible_without_log_close;
CREATE FUNCTION file_evidence_version_eligible(v BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT NOT object_log_close_pinned(v) AND file_evidence_version_eligible_without_log_close(v)
$$;
CREATE FUNCTION guard_object_log_close_ledger() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    -- Physical deletion ownership requires existing exclusive admission before
    -- ledger locks, including raw writes and already-deleting retries.
    IF NOT file_evidence_graph_held(true) AND (
        (EXISTS(SELECT 1 FROM log_segment WHERE object_key=OLD.object_key
            AND provider_version=OLD.provider_version) AND (TG_OP='DELETE' OR
            ROW(NEW.object_key,NEW.provider_version,NEW.size_bytes,NEW.state) IS DISTINCT FROM
            ROW(OLD.object_key,OLD.provider_version,OLD.size_bytes,OLD.state)))) THEN
        RAISE EXCEPTION 'Object deletion claim requires exclusive evidence admission';
    END IF;
    IF object_log_close_object_pinned(OLD.object_key,OLD.provider_version) THEN
        IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Object close pins provider object'; END IF;
        IF NEW IS DISTINCT FROM OLD THEN RAISE EXCEPTION 'Object close pins provider ledger identity'; END IF;
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER object_log_close_ledger_guard BEFORE UPDATE OR DELETE ON object_maintenance_ledger
    FOR EACH ROW EXECUTE FUNCTION guard_object_log_close_ledger();

CREATE FUNCTION guard_object_log_close_grant() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE s log_stream;
BEGIN
    SELECT * INTO s FROM log_stream WHERE artifact_version=NEW.artifact_version FOR UPDATE;
    IF s.close_phase IS NOT NULL THEN
        -- Only a no-op completed retry for the exact accepted provider object.
        IF NEW.state<>'completed' OR NOT EXISTS(SELECT 1 FROM log_segment g WHERE g.stream=s.id
            AND g.sequence=NEW.segment_sequence AND g.object_key=NEW.object_key
            AND g.size_bytes=NEW.expected_size AND g.sha256=NEW.expected_sha256
            AND g.provider_version=NEW.completed_provider_version) THEN
            RAISE EXCEPTION 'Object close forbids upload admission or changed completion';
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER object_log_close_grant_guard BEFORE INSERT OR UPDATE ON artifact_upload_grant
    FOR EACH ROW EXECUTE FUNCTION guard_object_log_close_grant();

ALTER FUNCTION file_evidence_request_cleanup(BIGINT[]) RENAME TO file_evidence_request_cleanup_without_log_close;
CREATE FUNCTION file_evidence_request_cleanup(requested BIGINT[]) RETURNS BIGINT[] LANGUAGE plpgsql AS $$
BEGIN
    PERFORM file_evidence_gc_entry();
    IF EXISTS(SELECT 1 FROM unnest(requested) v WHERE object_log_close_pinned(v)) THEN RETURN '{}'; END IF;
    RETURN file_evidence_request_cleanup_without_log_close(requested);
END $$;

CREATE FUNCTION guard_object_log_close_final_evidence() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM log_stream WHERE (id=NEW.stream OR artifact_version=NEW.artifact_version) AND close_phase IS NOT NULL) THEN
        RAISE EXCEPTION 'Selected close requires verified final evidence publication';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER object_log_close_final_evidence BEFORE INSERT ON runtime_log_final_manifest
    FOR EACH ROW EXECUTE FUNCTION guard_object_log_close_final_evidence();
