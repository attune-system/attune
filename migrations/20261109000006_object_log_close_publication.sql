-- Atomic selected-close publication. Rust additionally requires private verified
-- bytes and freshly acquired source authority; these constraints enforce shape.
CREATE OR REPLACE FUNCTION guard_object_log_close_stream() RETURNS TRIGGER LANGUAGE plpgsql AS $$
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
    IF OLD.close_phase='published' AND ROW(NEW.sealed,NEW.sealed_at,NEW.truncated)
       IS DISTINCT FROM ROW(OLD.sealed,OLD.sealed_at,OLD.truncated) THEN
        RAISE EXCEPTION 'Published selected seal is immutable';
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
            IF OLD.close_phase='closing' AND NEW.close_phase='published'
               AND NEW.close_revision=OLD.close_revision AND NEW.close_deadline=OLD.close_deadline
               AND OLD.close_deadline>clock_timestamp() AND NEW.sealed THEN
                PERFORM object_log_close_check_selection(OLD.id,OLD.close_input);
            ELSIF OLD.close_phase='closing' AND NEW.close_phase='abandoned'
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
    IF NEW.close_phase IS NOT NULL AND NEW.close_phase<>'published' AND ROW(NEW.sealed,NEW.sealed_at,NEW.truncated)
       IS DISTINCT FROM ROW(OLD.sealed,OLD.sealed_at,OLD.truncated) THEN
        RAISE EXCEPTION 'Selected close requires verified finalization';
    END IF;
    RETURN NEW;
END $$;
CREATE OR REPLACE FUNCTION guard_object_log_close_version() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF object_log_close_pinned(OLD.id) THEN RAISE EXCEPTION 'Object log close pins version'; END IF;
        RETURN OLD;
    END IF;
    IF EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
       AND (to_jsonb(NEW)-ARRAY['body_state','body_updated','legacy_snapshot_expires_at','size_bytes','sha256','provider_version','meta'])
           IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['body_state','body_updated','legacy_snapshot_expires_at','size_bytes','sha256','provider_version','meta']) THEN
        RAISE EXCEPTION 'Selected close forbids generic body mutation';
    END IF;
    IF OLD.body_state<>'pending' AND EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
       AND ROW(NEW.size_bytes,NEW.sha256,NEW.provider_version,NEW.meta)
           IS DISTINCT FROM ROW(OLD.size_bytes,OLD.sha256,OLD.provider_version,OLD.meta) THEN
        RAISE EXCEPTION 'Published selected result is immutable';
    END IF;
    -- Publication fields are checked against the selected input by deferred
    -- constraints on every participating table. No session bypass is available.
    IF EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
       AND ((COALESCE(NEW.meta,'{}'::jsonb)-'encrypted_log_manifest') IS DISTINCT FROM
            (COALESCE(OLD.meta,'{}'::jsonb)-'encrypted_log_manifest')) THEN
        RAISE EXCEPTION 'Selected close forbids metadata replacement';
    END IF;
    RETURN NEW;
END $$;
DROP TRIGGER object_log_close_final_evidence ON runtime_log_final_manifest;
CREATE FUNCTION check_object_log_close_publication() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE s log_stream; v artifact_version; sid BIGINT;
BEGIN
    IF TG_TABLE_NAME='log_stream' THEN sid=NEW.id;
    ELSIF TG_TABLE_NAME='artifact_version' THEN
        SELECT id INTO sid FROM log_stream WHERE artifact_version=NEW.id;
    ELSE sid=NEW.stream; END IF;
    SELECT * INTO s FROM log_stream WHERE id=sid;
    IF NOT FOUND OR s.close_phase IS NULL THEN RETURN NULL; END IF;
    SELECT * INTO STRICT v FROM artifact_version WHERE id=s.artifact_version;
    IF s.close_input->'snapshot' IS DISTINCT FROM object_log_close_snapshot(s.id) THEN
        RAISE EXCEPTION 'Selected membership changed';
    END IF;
    IF s.close_phase='published' THEN
        IF NOT s.sealed OR s.sealed_at IS NULL OR v.body_state NOT IN ('ready','cleanup_claimed','deleting')
           OR s.truncated IS DISTINCT FROM (s.close_input->>'truncated')::boolean
           OR v.size_bytes IS DISTINCT FROM s.total_bytes
           OR v.provider_version IS DISTINCT FROM 'segments:'||s.next_sequence::text
           OR v.sha256 IS NULL THEN RAISE EXCEPTION 'Partial selected publication'; END IF;
        IF s.close_input->'proof'->>'kind'='encrypted' THEN
            IF encode(sha256(convert_to(v.meta->>'encrypted_log_manifest','UTF8')),'hex')
                 IS DISTINCT FROM s.close_input->'proof'->>'authenticated_sha256'
               OR v.sha256 IS DISTINCT FROM s.close_input->'proof'->'manifest'->>'ciphertext_sha256' THEN
                RAISE EXCEPTION 'Selected authenticated metadata differs';
            END IF;
            IF s.close_input->'producer'->>'kind'='delivery' AND NOT EXISTS(
                SELECT 1 FROM runtime_log_final_manifest f WHERE f.stream=s.id AND f.artifact_version=v.id
                  AND f.cipher_bytes=v.size_bytes AND f.cipher_sha256=v.sha256
                  AND f.truncated=s.truncated AND f.authenticated_manifest=v.meta->>'encrypted_log_manifest') THEN
                RAISE EXCEPTION 'Selected final evidence missing';
            END IF;
        END IF;
    ELSIF s.sealed OR NOT (v.body_state='pending' OR (s.close_phase='abandoned' AND v.body_state IN ('cleanup_claimed','deleting'))) OR v.meta ? 'encrypted_log_manifest'
          OR EXISTS(SELECT 1 FROM runtime_log_final_manifest WHERE stream=s.id) THEN
        RAISE EXCEPTION 'Unpublished selection has final state';
    END IF;
    RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER object_log_publication_stream AFTER INSERT OR UPDATE ON log_stream
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_object_log_close_publication();
CREATE CONSTRAINT TRIGGER object_log_publication_version AFTER UPDATE ON artifact_version
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_object_log_close_publication();
CREATE CONSTRAINT TRIGGER object_log_publication_evidence AFTER INSERT OR UPDATE ON runtime_log_final_manifest
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_object_log_close_publication();
CREATE INDEX log_close_expired_discovery ON log_stream(close_deadline,id) WHERE close_phase='closing';
