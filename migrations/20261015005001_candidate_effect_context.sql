-- Candidate effects never enter production resource/event tables or their NOTIFY triggers.
CREATE TABLE pack_candidate_context (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    lease UUID NOT NULL UNIQUE,
    install_id BIGINT NOT NULL UNIQUE REFERENCES pack_install(id) ON DELETE CASCADE,
    worker_id BIGINT NOT NULL,
    archive_key TEXT NOT NULL,
    archive_version TEXT NOT NULL,
    archive_digest TEXT NOT NULL,
    archive_token_hash TEXT NOT NULL,
    runtime_token_hash TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    operations INTEGER NOT NULL DEFAULT 0 CHECK (operations BETWEEN 0 AND 1024)
);
CREATE TABLE pack_candidate_resource (
    context UUID NOT NULL REFERENCES pack_candidate_context(lease) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('key', 'trigger')),
    ref TEXT NOT NULL,
    value JSONB NOT NULL CHECK (octet_length(value::text) <= 65536),
    PRIMARY KEY (context, kind, ref)
);
CREATE TABLE pack_candidate_event (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    context UUID NOT NULL REFERENCES pack_candidate_context(lease) ON DELETE CASCADE,
    trigger_ref TEXT NOT NULL,
    payload JSONB NOT NULL CHECK (octet_length(payload::text) <= 65536),
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE FUNCTION guard_pack_candidate_context() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF (to_jsonb(NEW) - 'revoked' - 'operations' - 'expires_at') IS DISTINCT FROM
       (to_jsonb(OLD) - 'revoked' - 'operations' - 'expires_at') OR
       (OLD.revoked AND NOT NEW.revoked) OR NEW.expires_at > OLD.expires_at OR
       NEW.operations < OLD.operations THEN
        RAISE EXCEPTION 'candidate lease binding is immutable and cannot be renewed';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_pack_candidate_context BEFORE UPDATE ON pack_candidate_context
    FOR EACH ROW EXECUTE FUNCTION guard_pack_candidate_context();
CREATE FUNCTION close_pack_candidate_context() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.status <> 'running' THEN
        UPDATE pack_candidate_context SET revoked = TRUE WHERE install_id = NEW.id;
        DELETE FROM pack_candidate_resource WHERE context IN
            (SELECT lease FROM pack_candidate_context WHERE install_id = NEW.id);
        DELETE FROM pack_candidate_event WHERE context IN
            (SELECT lease FROM pack_candidate_context WHERE install_id = NEW.id);
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER pack_candidate_context_finish AFTER UPDATE OF status ON pack_install
    FOR EACH ROW EXECUTE FUNCTION close_pack_candidate_context();
