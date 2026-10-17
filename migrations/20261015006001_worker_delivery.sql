-- Worker-owned delivery is separate from execution API authority. No historical
-- execution is backfilled into this capability.
SET search_path TO attune, public;
CREATE TABLE worker_delivery_context (
    id BIGSERIAL PRIMARY KEY,
    token UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    execution BIGINT NOT NULL UNIQUE,
    admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE CASCADE,
    attempt UUID NOT NULL,
    worker BIGINT NOT NULL,
    worker_instance UUID NOT NULL,
    deadline TIMESTAMPTZ NOT NULL,
    CHECK (worker > 0),
    CHECK (attempt <> '00000000-0000-0000-0000-000000000000'::uuid),
    CHECK (worker_instance <> '00000000-0000-0000-0000-000000000000'::uuid)
);
CREATE TABLE worker_delivery_permit (
    id BIGSERIAL PRIMARY KEY,
    token UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    context UUID NOT NULL REFERENCES worker_delivery_context(token) ON DELETE CASCADE,
    artifact_version BIGINT NOT NULL UNIQUE REFERENCES artifact_version(id) ON DELETE CASCADE,
    log_stream BIGINT REFERENCES log_stream(id) ON DELETE CASCADE,
    max_bytes BIGINT NOT NULL CHECK (max_bytes >= 0),
    max_sequence BIGINT NOT NULL CHECK (max_sequence >= 0),
    owner_digest TEXT NOT NULL,
    publication_action TEXT NOT NULL CHECK (publication_action IN ('create','update'))
);
CREATE FUNCTION guard_worker_delivery_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'worker delivery capabilities are immutable';
END;
$$;
CREATE TRIGGER worker_delivery_context_immutable BEFORE UPDATE ON worker_delivery_context
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();
CREATE TRIGGER worker_delivery_permit_immutable BEFORE UPDATE ON worker_delivery_permit
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();
