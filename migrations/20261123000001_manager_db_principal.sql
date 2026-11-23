-- PostgreSQL logins may be bound to one already-issued manager session by the DBA.
-- Keep the binding after withdrawal so a login cannot claim a replacement boot.
CREATE TABLE manager_db_principal (
    principal OID PRIMARY KEY,
    login_name NAME NOT NULL UNIQUE,
    manager_session BIGINT NOT NULL UNIQUE REFERENCES manager_session(id),
    instance UUID NOT NULL,
    revoked_at TIMESTAMPTZ
);

REVOKE ALL ON manager_db_principal FROM PUBLIC;

-- Called only by the role-specific functions. SESSION_USER remains the
-- authenticated PostgreSQL principal inside their SECURITY DEFINER context.
CREATE FUNCTION inspect_manager_db_session(
    expected_role worker_role_enum, expected_instance UUID, withdraw BOOLEAN
) RETURNS TABLE (session_id BIGINT, expires_at TIMESTAMPTZ, closed_at TIMESTAMPTZ)
LANGUAGE plpgsql VOLATILE
AS $body$
DECLARE
    bound_session BIGINT;
    deadline TIMESTAMPTZ;
    withdrawn_at TIMESTAMPTZ;
BEGIN
    -- Recheck the binding under a shared lock before inspecting or withdrawing.
    SELECT binding.manager_session INTO bound_session
    FROM manager_db_principal binding
    JOIN pg_roles principal
      ON principal.oid = binding.principal AND principal.rolname = binding.login_name
    WHERE principal.rolname = SESSION_USER AND principal.rolcanlogin
        AND NOT principal.rolsuper AND NOT principal.rolcreatedb AND NOT principal.rolcreaterole
        AND NOT principal.rolreplication AND NOT principal.rolbypassrls
        AND binding.instance = expected_instance AND binding.revoked_at IS NULL
    FOR SHARE OF binding;
    IF NOT FOUND THEN
        RETURN;
    END IF;

    IF withdraw THEN
        SELECT s.expires_at, s.closed_at INTO deadline, withdrawn_at
        FROM manager_session s
        WHERE s.id = bound_session AND s.instance = expected_instance AND s.role = expected_role
        FOR UPDATE;
    ELSE
        SELECT s.expires_at, s.closed_at INTO deadline, withdrawn_at
        FROM manager_session s
        WHERE s.id = bound_session AND s.instance = expected_instance AND s.role = expected_role
        FOR SHARE;
    END IF;

    IF NOT FOUND OR deadline <= clock_timestamp() THEN
        RETURN;
    END IF;

    IF withdraw AND withdrawn_at IS NULL THEN
        UPDATE manager_session s SET closed_at = clock_timestamp()
        WHERE s.id = bound_session AND s.expires_at > clock_timestamp()
        RETURNING s.closed_at INTO withdrawn_at;
        IF NOT FOUND THEN
            RETURN;
        END IF;
    END IF;

    RETURN QUERY SELECT bound_session, deadline, withdrawn_at;
END
$body$;

CREATE FUNCTION manager_db_action_session(expected_instance UUID, withdraw BOOLEAN DEFAULT FALSE)
RETURNS TABLE (session_id BIGINT, expires_at TIMESTAMPTZ, closed_at TIMESTAMPTZ)
LANGUAGE sql VOLATILE STRICT SECURITY DEFINER
AS $body$
    SELECT session_id, expires_at, closed_at
    FROM inspect_manager_db_session('action', expected_instance, withdraw);
$body$;

CREATE FUNCTION manager_db_sensor_session(expected_instance UUID, withdraw BOOLEAN DEFAULT FALSE)
RETURNS TABLE (session_id BIGINT, expires_at TIMESTAMPTZ, closed_at TIMESTAMPTZ)
LANGUAGE sql VOLATILE STRICT SECURITY DEFINER
AS $body$
    SELECT session_id, expires_at, closed_at
    FROM inspect_manager_db_session('sensor', expected_instance, withdraw);
$body$;

REVOKE ALL ON FUNCTION inspect_manager_db_session(worker_role_enum, UUID, BOOLEAN),
    manager_db_action_session(UUID, BOOLEAN), manager_db_sensor_session(UUID, BOOLEAN) FROM PUBLIC;

-- The trusted migration schema must precede the caller's temporary schema.
DO $trust$
DECLARE
    signature TEXT;
BEGIN
    FOREACH signature IN ARRAY ARRAY[
        'inspect_manager_db_session(worker_role_enum, UUID, BOOLEAN)',
        'manager_db_action_session(UUID, BOOLEAN)',
        'manager_db_sensor_session(UUID, BOOLEAN)'
    ] LOOP
        EXECUTE format('ALTER FUNCTION %s SET search_path TO pg_catalog, %I, pg_temp',
            signature, current_schema());
    END LOOP;
END
$trust$;
