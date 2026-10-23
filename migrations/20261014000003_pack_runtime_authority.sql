-- Runtime sponsorship is independent of human installer/requester activity.
CREATE TABLE runtime_external_approval (
    id BIGSERIAL PRIMARY KEY,
    subject_pack BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    issuer_identity BIGINT NOT NULL CHECK (issuer_identity > 0),
    issuer_login TEXT NOT NULL CHECK (length(issuer_login) > 0),
    grants JSONB NOT NULL CHECK (jsonb_typeof(grants) = 'array'),
    approval_digest TEXT NOT NULL CHECK (approval_digest ~ '^[0-9a-f]{64}$'),
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX runtime_external_approval_subject_idx
    ON runtime_external_approval(subject_pack, id);

CREATE TABLE pack_runtime_policy_revision (
    id BIGSERIAL PRIMARY KEY,
    pack BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    release BIGINT NOT NULL REFERENCES pack_release(id) ON DELETE CASCADE,
    policy JSONB NOT NULL CHECK (jsonb_typeof(policy) = 'object'),
    policy_digest TEXT NOT NULL CHECK (policy_digest ~ '^[0-9a-f]{64}$'),
    revoked_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX pack_runtime_policy_live_digest_idx
    ON pack_runtime_policy_revision(pack, release, policy_digest) WHERE revoked_at IS NULL;
CREATE INDEX pack_runtime_policy_release_idx ON pack_runtime_policy_revision(release);

CREATE TABLE pack_runtime_policy_current (
    pack BIGINT PRIMARY KEY REFERENCES pack(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE CASCADE
);

-- These runtime IDs deliberately have no FK. Execution/enforcement retention is independent.
CREATE TABLE runtime_authority_admission (
    id BIGSERIAL PRIMARY KEY,
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('execution', 'enforcement')),
    owner_id BIGINT NOT NULL CHECK (owner_id > 0),
    policy_revision BIGINT NOT NULL CHECK (policy_revision > 0),
    authority JSONB NOT NULL CHECK (jsonb_typeof(authority) = 'object'),
    authority_digest TEXT NOT NULL CHECK (authority_digest ~ '^[0-9a-f]{64}$'),
    revoked_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (owner_kind, owner_id)
);
CREATE INDEX runtime_authority_admission_policy_idx
    ON runtime_authority_admission(policy_revision);

CREATE FUNCTION guard_runtime_authority_record() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (to_jsonb(NEW) - 'revoked_at') IS DISTINCT FROM (to_jsonb(OLD) - 'revoked_at') THEN
        RAISE EXCEPTION 'runtime authority records are immutable';
    END IF;
    IF OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS DISTINCT FROM OLD.revoked_at THEN
        RAISE EXCEPTION 'runtime authority revocation cannot be reversed';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_runtime_external_approval BEFORE UPDATE ON runtime_external_approval
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_authority_record();
CREATE TRIGGER guard_pack_runtime_policy BEFORE UPDATE ON pack_runtime_policy_revision
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_authority_record();
CREATE TRIGGER guard_runtime_authority_admission BEFORE UPDATE ON runtime_authority_admission
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_authority_record();

CREATE FUNCTION delete_runtime_authority_admission() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM runtime_authority_admission WHERE owner_kind = TG_TABLE_NAME AND owner_id = OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER delete_execution_runtime_authority AFTER DELETE ON execution
    FOR EACH ROW EXECUTE FUNCTION delete_runtime_authority_admission();
CREATE TRIGGER delete_enforcement_runtime_authority AFTER DELETE ON enforcement
    FOR EACH ROW EXECUTE FUNCTION delete_runtime_authority_admission();

COMMENT ON COLUMN runtime_external_approval.issuer_identity IS
    'Historical issuing human ID, not a live authentication or runtime-authorization dependency';
COMMENT ON COLUMN runtime_authority_admission.policy_revision IS
    'Immutable authority revision ID; missing revisions fail closed, independently of runtime retention';
