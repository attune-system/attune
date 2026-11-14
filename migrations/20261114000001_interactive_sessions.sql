-- Forward, data-preserving transition: pre-session interactive refresh JWTs fail closed.
CREATE TABLE interactive_session (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    identity_id BIGINT NOT NULL REFERENCES identity(id) ON DELETE CASCADE,
    current_hash TEXT NOT NULL UNIQUE,
    expires TIMESTAMPTZ NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE INDEX interactive_session_identity ON interactive_session(identity_id);
CREATE TABLE interactive_refresh_use (
    token_hash TEXT PRIMARY KEY,
    session_id BIGINT NOT NULL REFERENCES interactive_session(id) ON DELETE CASCADE
);
CREATE TABLE interactive_login_exchange (
    code_hash TEXT PRIMARY KEY,
    browser_hash TEXT NOT NULL,
    identity_id BIGINT NOT NULL REFERENCES identity(id) ON DELETE CASCADE,
    payload TEXT NOT NULL,
    redirect_to TEXT NOT NULL,
    expires TIMESTAMPTZ NOT NULL DEFAULT (clock_timestamp() + interval '60 seconds')
);
CREATE FUNCTION revoke_interactive_password_sessions() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    UPDATE interactive_session SET revoked = TRUE WHERE identity_id = NEW.id;
    DELETE FROM interactive_login_exchange WHERE identity_id = NEW.id;
    RETURN NEW;
END;
$$;
CREATE TRIGGER identity_password_session_revocation AFTER UPDATE OF password_hash ON identity
FOR EACH ROW WHEN (OLD.password_hash IS DISTINCT FROM NEW.password_hash)
EXECUTE FUNCTION revoke_interactive_password_sessions();
