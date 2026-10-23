-- Sink snapshots reuse C's immutable graph vocabulary, but are not process reads.
ALTER TABLE execution_runtime_read_origin ADD COLUMN file_sink_version BIGINT;
ALTER TABLE execution_runtime_read_origin DROP CONSTRAINT execution_runtime_read_origin_admission_origin_digest_key;
CREATE UNIQUE INDEX execution_runtime_read_acquisition_digest ON execution_runtime_read_origin(admission,origin_digest)
    WHERE file_sink_version IS NULL;
CREATE UNIQUE INDEX execution_runtime_read_file_sink ON execution_runtime_read_origin(file_sink_version)
    WHERE file_sink_version IS NOT NULL;

CREATE TABLE artifact_file_origin (
    artifact_version BIGINT PRIMARY KEY REFERENCES artifact_version(id) ON DELETE CASCADE,
    origin_read BIGINT NOT NULL UNIQUE,
    admission BIGINT NOT NULL,
    execution BIGINT NOT NULL,
    attempt UUID NOT NULL,
    sponsor BIGINT NOT NULL,
    max_plaintext_bytes BIGINT NOT NULL CHECK (max_plaintext_bytes >= 0),
    source_digest TEXT NOT NULL,
    receipts JSONB NOT NULL CHECK (octet_length(receipts::TEXT) <= 1048576),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE TRIGGER artifact_file_origin_immutable BEFORE UPDATE ON artifact_file_origin
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();

CREATE TABLE artifact_file_body (
    artifact_version BIGINT PRIMARY KEY REFERENCES artifact_version(id) ON DELETE CASCADE,
    origin_read BIGINT NOT NULL,
    plaintext_bytes BIGINT NOT NULL CHECK (plaintext_bytes >= 0),
    plaintext_sha256 TEXT NOT NULL CHECK (length(plaintext_sha256) = 64),
    ciphertext_bytes BIGINT NOT NULL CHECK (ciphertext_bytes >= 0),
    ciphertext_sha256 TEXT NOT NULL CHECK (length(ciphertext_sha256) = 64),
    authenticated_manifest TEXT NOT NULL
);
CREATE TRIGGER artifact_file_body_immutable BEFORE UPDATE ON artifact_file_body
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();

-- Pending encrypted bytes make retries identical across API instances. Deleted
-- with the Ready commit; never returned in artifact DTOs or public blob routes.
CREATE TABLE artifact_file_upload (
    artifact_version BIGINT PRIMARY KEY REFERENCES artifact_version(id) ON DELETE CASCADE,
    ciphertext BYTEA NOT NULL
);
CREATE TRIGGER artifact_file_upload_immutable BEFORE UPDATE ON artifact_file_upload
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();

CREATE FUNCTION guard_file_proof_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM artifact_version WHERE id=OLD.artifact_version) THEN
        RAISE EXCEPTION 'File origin/body proof may only be removed with its version';
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER artifact_file_origin_delete_guard BEFORE DELETE ON artifact_file_origin
FOR EACH ROW EXECUTE FUNCTION guard_file_proof_delete();
CREATE TRIGGER artifact_file_body_delete_guard BEFORE DELETE ON artifact_file_body
FOR EACH ROW EXECUTE FUNCTION guard_file_proof_delete();

CREATE FUNCTION guard_file_origin_version() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM artifact_file_origin WHERE artifact_version=OLD.id) AND
       (NEW.artifact IS DISTINCT FROM OLD.artifact OR NEW.execution IS DISTINCT FROM OLD.execution
        OR NEW.file_path IS DISTINCT FROM OLD.file_path OR NEW.object_key IS DISTINCT FROM OLD.object_key
        OR NEW.content IS DISTINCT FROM OLD.content OR NEW.content_json IS DISTINCT FROM OLD.content_json
        OR (OLD.body_state='ready' AND
            (COALESCE(NEW.body_state::TEXT,'') NOT IN ('ready','deleting','cleanup_claimed')
             OR NEW.provider_version IS DISTINCT FROM OLD.provider_version
             OR NEW.size_bytes IS DISTINCT FROM OLD.size_bytes OR NEW.sha256 IS DISTINCT FROM OLD.sha256))) THEN
        RAISE EXCEPTION 'Origin-bound file body cannot be rebound or replaced';
    END IF;
    IF NEW.body_state='ready' AND EXISTS (SELECT 1 FROM artifact_file_origin WHERE artifact_version=NEW.id)
       AND NOT EXISTS (SELECT 1 FROM artifact_file_body b JOIN artifact_file_origin o USING (artifact_version)
                       WHERE b.artifact_version=NEW.id AND b.origin_read=o.origin_read
                       AND b.ciphertext_bytes=NEW.size_bytes AND b.ciphertext_sha256=NEW.sha256) THEN
        RAISE EXCEPTION 'Ready file requires its exact encrypted body proof';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER file_origin_version_guard BEFORE UPDATE ON artifact_version
FOR EACH ROW EXECUTE FUNCTION guard_file_origin_version();

CREATE FUNCTION guard_file_artifact_body_kind() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.type IS DISTINCT FROM OLD.type OR NEW.classification IS DISTINCT FROM OLD.classification)
       AND EXISTS (SELECT 1 FROM artifact_file_origin o JOIN artifact_version v ON v.id=o.artifact_version
                   WHERE v.artifact=OLD.id) THEN
        RAISE EXCEPTION 'Origin-bound file artifacts cannot change body kind or classification';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER file_artifact_body_kind_guard BEFORE UPDATE ON artifact
FOR EACH ROW EXECUTE FUNCTION guard_file_artifact_body_kind();
