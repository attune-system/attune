-- Close NULL/INSERT/deletion holes in the selected object-log lifecycle.
CREATE OR REPLACE FUNCTION guard_object_log_publication_identity() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='log_stream' THEN
        IF ROW(NEW.backend,NEW.artifact_version,NEW.id) IS DISTINCT FROM
           ROW(OLD.backend,OLD.artifact_version,OLD.id) THEN
            RAISE EXCEPTION 'Log stream binding and backend are immutable';
        END IF;
    ELSIF EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
          AND ROW(NEW.size_bytes,NEW.sha256,NEW.provider_version) IS DISTINCT FROM
              ROW(OLD.size_bytes,OLD.sha256,OLD.provider_version)
          AND (OLD.body_state='pending' AND NEW.body_state='ready' AND EXISTS(
              SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase='published')) IS NOT TRUE THEN
        RAISE EXCEPTION 'Selected bytes may change only in atomic publication';
    END IF;
    RETURN NEW;
END $$;

-- Normalize absent operational metadata before comparing immutable identity.
-- Preserve PR31 per-stream delivery status after byte publication. These two
-- operational fields are not selected source/proof/body identity.
CREATE OR REPLACE FUNCTION guard_object_log_close_version() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF object_log_close_pinned(OLD.id) THEN RAISE EXCEPTION 'Object log close pins version'; END IF;
        RETURN OLD;
    END IF;
    IF NEW.body_state IS NULL AND EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL) THEN
        RAISE EXCEPTION 'Selected log body state cannot be NULL';
    END IF;
    IF EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
       AND (to_jsonb(NEW)-ARRAY['body_state','body_updated','legacy_snapshot_expires_at','size_bytes','sha256','provider_version','meta'])
           IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['body_state','body_updated','legacy_snapshot_expires_at','size_bytes','sha256','provider_version','meta']) THEN
        RAISE EXCEPTION 'Selected close forbids generic body mutation';
    END IF;
    IF OLD.body_state IS DISTINCT FROM 'pending' AND EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
       AND ROW(NEW.size_bytes,NEW.sha256,NEW.provider_version,COALESCE(NEW.meta,'{}'::jsonb)-ARRAY['log_state','log_failure'])
           IS DISTINCT FROM ROW(OLD.size_bytes,OLD.sha256,OLD.provider_version,COALESCE(OLD.meta,'{}'::jsonb)-ARRAY['log_state','log_failure']) THEN
        RAISE EXCEPTION 'Published selected result is immutable';
    END IF;
    -- Publication fields are checked against the selected input by deferred
    -- constraints on every participating table. No session bypass is available.
    IF EXISTS(SELECT 1 FROM log_stream WHERE artifact_version=OLD.id AND close_phase IS NOT NULL)
       AND ((COALESCE(NEW.meta,'{}'::jsonb)-ARRAY['encrypted_log_manifest','log_state','log_failure']) IS DISTINCT FROM
            (COALESCE(OLD.meta,'{}'::jsonb)-ARRAY['encrypted_log_manifest','log_state','log_failure'])) THEN
        RAISE EXCEPTION 'Selected close forbids metadata replacement';
    END IF;
    RETURN NEW;
END $$;

-- Match every recorded final-evidence attribute to authenticated selection.
CREATE OR REPLACE FUNCTION check_object_log_close_publication() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE s log_stream; v artifact_version; sid BIGINT;
BEGIN
    IF TG_TABLE_NAME='log_stream' THEN sid=NEW.id;
    ELSIF TG_TABLE_NAME='artifact_version' THEN
        SELECT id INTO sid FROM log_stream WHERE artifact_version=NEW.id;
    ELSIF TG_OP='DELETE' THEN sid=OLD.stream;
    ELSE sid=NEW.stream; END IF;
    SELECT * INTO s FROM log_stream WHERE id=sid;
    IF NOT FOUND THEN RETURN NULL; END IF;
    SELECT * INTO STRICT v FROM artifact_version WHERE id=s.artifact_version;
    IF s.backend='shared_file' THEN RETURN NULL; END IF;
    IF s.close_phase IS NULL THEN
        IF s.sealed OR v.body_state IS NULL OR v.body_state NOT IN ('pending','cleanup_claimed','deleting')
           OR EXISTS(SELECT 1 FROM runtime_log_final_manifest WHERE stream=s.id) THEN
            RAISE EXCEPTION 'Object log final state requires selected publication';
        END IF;
        RETURN NULL;
    END IF;
    IF v.body_state IS NULL THEN RAISE EXCEPTION 'Selected log body state cannot be NULL'; END IF;
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
                  AND f.truncated=s.truncated AND f.authenticated_manifest=v.meta->>'encrypted_log_manifest'
                  AND f.execution=(s.close_input->'producer'->>'execution')::bigint
                  AND f.admission=(s.close_input->'producer'->>'admission')::bigint
                  AND f.attempt=(s.close_input->'producer'->>'attempt')::uuid
                  AND f.worker=(s.close_input->'producer'->>'worker')::bigint
                  AND f.worker_instance=(s.close_input->'producer'->>'instance')::uuid
                  AND f.encoding=s.close_input->'proof'->'manifest'->>'encoding'
                  AND f.frame_count=(s.close_input->'proof'->'manifest'->>'frame_count')::bigint
                  AND f.plain_bytes=(s.close_input->'proof'->'manifest'->>'plaintext_bytes')::bigint) THEN
                RAISE EXCEPTION 'Selected final evidence missing';
            END IF;
        END IF;
    ELSIF s.sealed OR NOT (v.body_state='pending' OR (s.close_phase='abandoned' AND v.body_state IN ('cleanup_claimed','deleting'))) OR v.meta ? 'encrypted_log_manifest'
          OR EXISTS(SELECT 1 FROM runtime_log_final_manifest WHERE stream=s.id) THEN
        RAISE EXCEPTION 'Unpublished selection has final state';
    END IF;
    RETURN NULL;
END $$;

-- Check reverse creation order too; object streams always start open and pending.
CREATE FUNCTION guard_object_log_initialization() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.backend='object_segments' AND
       (NEW.sealed OR NEW.sealed_at IS NOT NULL OR NEW.next_sequence<>0 OR NEW.total_bytes<>0
        OR NOT EXISTS(SELECT 1 FROM artifact_version WHERE id=NEW.artifact_version AND body_state='pending' FOR SHARE)) THEN
        RAISE EXCEPTION 'Object log stream must start open on a pending version';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER object_log_initialization BEFORE INSERT ON log_stream
    FOR EACH ROW EXECUTE FUNCTION guard_object_log_initialization();

-- Cascades see their version already removed. Standalone published membership
-- removal must not drop the references that protect accepted provider objects.
CREATE FUNCTION guard_published_object_log_segment_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM log_stream s JOIN artifact_version v ON v.id=s.artifact_version
              WHERE s.id=OLD.stream AND s.close_phase IN ('closing','published')) THEN
        RAISE EXCEPTION 'Retained selected log membership cannot be deleted';
    END IF;
    RETURN OLD;
END $$;
CREATE TRIGGER object_log_retained_membership BEFORE DELETE ON log_segment
    FOR EACH ROW EXECUTE FUNCTION guard_published_object_log_segment_delete();
CREATE TRIGGER object_log_retained_final_evidence BEFORE DELETE ON runtime_log_final_manifest
    FOR EACH ROW EXECUTE FUNCTION guard_published_object_log_segment_delete();

-- Validate evidence removal against the surviving published selection as well
-- as insert/update; teardown cascades have no surviving stream to validate.
CREATE CONSTRAINT TRIGGER object_log_publication_evidence_delete AFTER DELETE ON runtime_log_final_manifest
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_object_log_close_publication();
