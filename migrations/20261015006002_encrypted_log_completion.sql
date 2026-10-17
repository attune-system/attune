SET search_path TO attune, public;

ALTER TABLE worker_delivery_permit ADD COLUMN max_plaintext_bytes BIGINT
    CHECK (max_plaintext_bytes IS NULL OR max_plaintext_bytes >= 0);

CREATE TABLE runtime_log_final_manifest (
    artifact_version BIGINT PRIMARY KEY REFERENCES artifact_version(id) ON DELETE CASCADE,
    stream BIGINT NOT NULL UNIQUE REFERENCES log_stream(id) ON DELETE CASCADE,
    execution BIGINT NOT NULL CHECK (execution > 0),
    admission BIGINT NOT NULL CHECK (admission > 0),
    attempt UUID NOT NULL,
    worker BIGINT NOT NULL CHECK (worker > 0),
    worker_instance UUID NOT NULL,
    encoding TEXT NOT NULL CHECK (encoding = 'attune_encrypted_log_v1'),
    frame_count BIGINT NOT NULL CHECK (frame_count >= 0),
    plain_bytes BIGINT NOT NULL CHECK (plain_bytes >= 0),
    cipher_bytes BIGINT NOT NULL CHECK (cipher_bytes >= 0),
    cipher_sha256 TEXT NOT NULL CHECK (cipher_sha256 ~ '^[0-9a-f]{64}$'),
    truncated BOOLEAN NOT NULL,
    authenticated_manifest TEXT NOT NULL CHECK (octet_length(authenticated_manifest) BETWEEN 1 AND 8192),
    sealed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE TRIGGER runtime_log_final_manifest_immutable BEFORE UPDATE ON runtime_log_final_manifest
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();

CREATE FUNCTION guard_encrypted_log_manifest_metadata() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.body_state = 'ready' AND NEW.meta->>'encrypted_log_manifest'
       IS DISTINCT FROM OLD.meta->>'encrypted_log_manifest' THEN
        RAISE EXCEPTION 'Ready encrypted log final manifest cannot be added or replaced';
    END IF;
    IF OLD.meta->>'encrypted_log_manifest' IS NOT NULL AND
       (NEW.meta->>'encrypted_log_manifest' IS DISTINCT FROM OLD.meta->>'encrypted_log_manifest'
        OR NEW.meta->>'log_encoding' IS DISTINCT FROM OLD.meta->>'log_encoding') THEN
        RAISE EXCEPTION 'Encrypted log final manifest cannot be replaced';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER encrypted_log_manifest_metadata_immutable BEFORE UPDATE ON artifact_version
FOR EACH ROW EXECUTE FUNCTION guard_encrypted_log_manifest_metadata();

COMMENT ON TABLE runtime_log_final_manifest IS
    'Authenticated complete encrypted stream proof, committed atomically with seal/Ready. Historical execution/admission attribution does not block independent retention.';
