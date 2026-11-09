-- Match every recorded final-evidence attribute to authenticated selection.
CREATE OR REPLACE FUNCTION check_object_log_close_publication() RETURNS TRIGGER LANGUAGE plpgsql AS $$
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
