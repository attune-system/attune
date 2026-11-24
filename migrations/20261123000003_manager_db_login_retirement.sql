-- Only the API writes verified requests. The supervisor owns SQL role DDL
-- through guarded operations, including a retirement fence independent of the
-- rows locked by a manager's own SQL transactions.
CREATE TABLE manager_db_provisioning (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    login_prefix TEXT NOT NULL CHECK (login_prefix ~ '^[a-z][a-z0-9_]{0,27}$'),
    action_group NAME NOT NULL,
    sensor_group NAME NOT NULL CHECK (sensor_group <> action_group)
);

CREATE TABLE manager_db_credential (
    manager_session BIGINT PRIMARY KEY REFERENCES manager_session(id),
    pod_uid UUID NOT NULL,
    pod_invalidated BOOLEAN NOT NULL DEFAULT FALSE,
    pod_check_after TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    request_id UUID NOT NULL,
    requested_generation BIGINT NOT NULL CHECK (requested_generation > 0),
    installed_generation BIGINT NOT NULL DEFAULT 0,
    proof_expires_at TIMESTAMPTZ NOT NULL,
    pending_verifier TEXT,
    principal OID UNIQUE,
    login_name NAME UNIQUE,
    credential_expires_at TIMESTAMPTZ,
    retired_at TIMESTAMPTZ,
    drained_at TIMESTAMPTZ,
    install_retry_after TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    disable_retry_after TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    drain_retry_after TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    CHECK (installed_generation BETWEEN 0 AND requested_generation),
    CHECK ((principal IS NULL) = (login_name IS NULL))
);

REVOKE ALL ON manager_db_provisioning, manager_db_credential FROM PUBLIC;

CREATE INDEX manager_db_credential_active ON manager_db_credential (manager_session)
    WHERE drained_at IS NULL;


-- Apply the same effective grant checks to manual and managed logins.
CREATE FUNCTION valid_manager_db_role_privileges(principal_oid OID, requested_role worker_role_enum)
RETURNS BOOLEAN
LANGUAGE sql STABLE STRICT
AS $body$
    SELECT NOT has_schema_privilege(principal_oid, catalog.relnamespace, 'CREATE')
        AND NOT EXISTS (
            SELECT 1 FROM pg_class relation
            WHERE relation.relnamespace = catalog.relnamespace
                AND CASE
                    WHEN relation.relkind = 'S' THEN
                        has_sequence_privilege(principal_oid, relation.oid, 'USAGE, SELECT, UPDATE')
                    WHEN relation.relkind IN ('r', 'p', 'v', 'm', 'f') THEN
                        has_any_column_privilege(principal_oid, relation.oid, 'SELECT, INSERT, UPDATE, REFERENCES')
                        OR has_table_privilege(principal_oid, relation.oid, 'DELETE, TRUNCATE, TRIGGER')
                    ELSE FALSE
                END
        )
        AND has_function_privilege(principal_oid, 'manager_db_action_session(uuid,boolean)', 'EXECUTE')
            = (requested_role = 'action')
        AND has_function_privilege(principal_oid, 'manager_db_sensor_session(uuid,boolean)', 'EXECUTE')
            = (requested_role = 'sensor')
        AND NOT has_function_privilege(
            principal_oid, 'inspect_manager_db_session(worker_role_enum,uuid,boolean)', 'EXECUTE')
    FROM pg_class catalog
    WHERE catalog.oid = 'manager_db_principal'::regclass
$body$;

CREATE FUNCTION install_requested_manager_db_logins(batch_size INTEGER)
RETURNS INTEGER
LANGUAGE plpgsql VOLATILE STRICT SECURITY DEFINER
SET createrole_self_grant = 'inherit'
AS $body$
DECLARE
    request RECORD;
    profile manager_db_provisioning%ROWTYPE;
    scope RECORD;
    role_name NAME;
    capability NAME;
    login_oid OID;
    deadline TIMESTAMPTZ;
    installed_count INTEGER := 0;
BEGIN
    SELECT * INTO STRICT profile FROM manager_db_provisioning;
    FOR request IN
        SELECT * FROM manager_db_credential
        WHERE drained_at IS NULL AND retired_at IS NULL AND NOT pod_invalidated AND pending_verifier IS NOT NULL
          AND proof_expires_at > statement_timestamp()
          AND install_retry_after <= statement_timestamp()
        ORDER BY install_retry_after, manager_session
        LIMIT greatest(0, least(batch_size, 32))
        FOR UPDATE SKIP LOCKED
    LOOP
        -- Leave time to commit completed rows before the supervisor's
        -- statement deadline. Each login rolls back through its own savepoint.
        EXIT WHEN clock_timestamp() >= statement_timestamp() + interval '3 seconds';
        BEGIN
            -- The API locks its session before recording a request. Skip this
            -- operation if an API update or a manager-held transaction owns it.
            SELECT id, instance, role INTO scope FROM manager_session
            WHERE id = request.manager_session AND closed_at IS NULL
              AND expires_at > clock_timestamp()
            FOR SHARE SKIP LOCKED;
            IF NOT FOUND THEN
                UPDATE manager_db_credential SET install_retry_after = clock_timestamp() + interval '2 seconds'
                WHERE manager_session = request.manager_session;
                CONTINUE;
            END IF;
            deadline := request.proof_expires_at;
            IF deadline <= clock_timestamp() THEN
                CONTINUE;
            END IF;
            IF request.pending_verifier !~ '^SCRAM-SHA-256\$4096:[A-Za-z0-9+/]{22}==\$[A-Za-z0-9+/]{43}=:[A-Za-z0-9+/]{43}=$' THEN
                RAISE EXCEPTION 'Invalid manager SCRAM verifier';
            END IF;

            role_name := (profile.login_prefix || '_' || scope.id)::NAME;
            capability := CASE scope.role
                WHEN 'action' THEN profile.action_group
                WHEN 'sensor' THEN profile.sensor_group
            END;
            IF request.principal IS NULL THEN
                EXECUTE format('CREATE ROLE %I NOLOGIN CONNECTION LIMIT 2', role_name);
                EXECUTE format('GRANT %I TO %I WITH INHERIT TRUE, SET FALSE, ADMIN FALSE',
                               capability, role_name);
                SELECT oid INTO STRICT login_oid FROM pg_roles WHERE rolname = role_name;
                INSERT INTO manager_db_principal (principal, login_name, manager_session, instance)
                VALUES (login_oid, role_name, scope.id, scope.instance);
            ELSE
                login_oid := request.principal;
                IF NOT EXISTS (
                    SELECT 1 FROM pg_roles r
                    JOIN pg_auth_members m ON m.roleid = r.oid
                    WHERE r.oid = login_oid AND r.rolname = role_name AND r.rolcanlogin
                      AND NOT (r.rolsuper OR r.rolcreaterole OR r.rolcreatedb
                               OR r.rolreplication OR r.rolbypassrls)
                      AND m.member = current_user::regrole AND m.admin_option
                      AND pg_has_role(r.oid, 'USAGE')
                ) THEN
                    RAISE EXCEPTION 'Unexpected manager SQL role state';
                END IF;
            END IF;
            IF NOT coalesce(valid_manager_db_role_privileges(login_oid, scope.role), false) THEN
                RAISE EXCEPTION 'Invalid effective privileges for scoped manager login';
            END IF;
            BEGIN
                EXECUTE format('ALTER ROLE %I LOGIN PASSWORD %L VALID UNTIL %L',
                               role_name, request.pending_verifier, deadline);
            EXCEPTION WHEN OTHERS THEN
                -- Discard the dynamic statement context containing the verifier.
                RAISE EXCEPTION 'Unable to install manager password (SQLSTATE %)', SQLSTATE;
            END;
            UPDATE manager_db_credential SET principal = login_oid, login_name = role_name,
                installed_generation = request.requested_generation,
                credential_expires_at = deadline, pending_verifier = NULL,
                install_retry_after = '-infinity'
            WHERE manager_session = request.manager_session;
            installed_count := installed_count + 1;
        EXCEPTION WHEN OTHERS THEN
            UPDATE manager_db_credential SET install_retry_after = clock_timestamp() + interval '15 seconds'
            WHERE manager_session = request.manager_session;
            RAISE WARNING 'Manager database install failed for session % (SQLSTATE %); will retry',
                request.manager_session, SQLSTATE;
        END;
    END LOOP;
    RETURN installed_count;
END
$body$;

CREATE FUNCTION disable_retired_manager_db_logins(batch_size INTEGER)
RETURNS INTEGER
LANGUAGE plpgsql VOLATILE STRICT SECURITY DEFINER
AS $body$
DECLARE
    binding RECORD;
    retired_count INTEGER := 0;
BEGIN
    FOR binding IN
        SELECT c.manager_session, c.principal, c.login_name
        FROM manager_db_credential c JOIN manager_session s ON s.id = c.manager_session
        WHERE c.drained_at IS NULL AND c.retired_at IS NULL
          AND c.disable_retry_after <= statement_timestamp()
          AND (c.pod_invalidated OR s.closed_at IS NOT NULL OR s.expires_at <= statement_timestamp()
               OR c.credential_expires_at <= statement_timestamp())
        ORDER BY c.disable_retry_after, c.manager_session
        LIMIT greatest(0, least(batch_size, 32))
        FOR UPDATE OF c SKIP LOCKED
    LOOP
        -- Leave time to commit completed rows before the supervisor's
        -- statement deadline. Each login rolls back through its own savepoint.
        EXIT WHEN clock_timestamp() >= statement_timestamp() + interval '3 seconds';
        BEGIN
            IF EXISTS (SELECT 1 FROM pg_roles WHERE oid = binding.principal) THEN
                IF NOT EXISTS (
                    SELECT 1 FROM pg_roles r
                    JOIN pg_auth_members m ON m.roleid = r.oid
                    WHERE r.oid = binding.principal AND r.rolname = binding.login_name
                      AND NOT (r.rolsuper OR r.rolcreaterole OR r.rolcreatedb
                               OR r.rolreplication OR r.rolbypassrls)
                      AND m.member = current_user::regrole AND m.admin_option
                      AND pg_has_role(r.oid, 'USAGE')
                ) THEN
                    RAISE EXCEPTION 'Managed manager login no longer has expected ownership';
                END IF;
                EXECUTE format('ALTER ROLE %I NOLOGIN PASSWORD NULL', binding.login_name);
            END IF;
            UPDATE manager_db_credential SET retired_at = clock_timestamp(),
                pending_verifier = NULL
            WHERE manager_session = binding.manager_session;
            retired_count := retired_count + 1;
        EXCEPTION WHEN OTHERS THEN
            UPDATE manager_db_credential SET disable_retry_after = clock_timestamp() + interval '15 seconds'
            WHERE manager_session = binding.manager_session;
            RAISE WARNING 'Manager database disable failed for session % (SQLSTATE %); will retry',
                binding.manager_session, SQLSTATE;
        END;
    END LOOP;
    -- Clear verifier material from abandoned requests, including those which
    -- never reached issuance. Preserve their generation for an exact retry.
    UPDATE manager_db_credential SET pending_verifier = NULL
    WHERE manager_session IN (
        SELECT manager_session FROM manager_db_credential
        WHERE drained_at IS NULL AND pending_verifier IS NOT NULL
          AND proof_expires_at <= statement_timestamp()
        ORDER BY manager_session LIMIT greatest(0, least(batch_size, 32))
        FOR UPDATE SKIP LOCKED
    );
    RETURN retired_count;
END
$body$;

CREATE FUNCTION drain_retired_manager_db_connections(batch_size INTEGER)
RETURNS INTEGER
LANGUAGE plpgsql VOLATILE STRICT SECURITY DEFINER
AS $body$
DECLARE
    binding RECORD;
    connection_pid INTEGER;
    drained_count INTEGER := 0;
BEGIN
    FOR binding IN
        SELECT manager_session, principal, login_name, retired_at
        FROM manager_db_credential
        WHERE retired_at IS NOT NULL AND drained_at IS NULL
          AND drain_retry_after <= statement_timestamp()
        ORDER BY drain_retry_after, manager_session
        LIMIT greatest(0, least(batch_size, 32))
        FOR UPDATE SKIP LOCKED
    LOOP
        -- Leave time to commit completed rows before the supervisor's
        -- statement deadline. Each login rolls back through its own savepoint.
        EXIT WHEN clock_timestamp() >= statement_timestamp() + interval '3 seconds';
        BEGIN
            IF EXISTS (SELECT 1 FROM pg_roles WHERE oid = binding.principal) AND NOT EXISTS (
                SELECT 1 FROM pg_roles r
                JOIN pg_auth_members m ON m.roleid = r.oid
                WHERE r.oid = binding.principal AND r.rolname = binding.login_name
                  AND NOT (r.rolcanlogin OR r.rolsuper OR r.rolcreaterole OR r.rolcreatedb
                           OR r.rolreplication OR r.rolbypassrls)
                  AND m.member = current_user::regrole AND m.admin_option
                      AND pg_has_role(r.oid, 'USAGE')
            ) THEN
                RAISE EXCEPTION 'Managed manager login no longer has expected retirement state';
            END IF;
            PERFORM pg_stat_clear_snapshot();
            IF EXISTS (SELECT 1 FROM pg_stat_activity WHERE usesysid = binding.principal
                       AND backend_type = 'client backend') THEN
                FOR connection_pid IN
                    SELECT pid FROM pg_stat_activity WHERE usesysid = binding.principal
                      AND backend_type = 'client backend'
                    ORDER BY pid LIMIT 32
                LOOP
                    PERFORM pg_terminate_backend(connection_pid);
                END LOOP;
            ELSIF binding.principal IS NULL OR
                  binding.retired_at + current_setting('authentication_timeout')::interval
                  <= clock_timestamp() THEN
                -- A caller can hold this row lock until its backend exits.
                UPDATE manager_db_principal SET revoked_at = coalesce(revoked_at, clock_timestamp())
                WHERE principal = binding.principal;
                IF EXISTS (SELECT 1 FROM pg_roles WHERE oid = binding.principal) THEN
                    BEGIN
                        EXECUTE format('DROP ROLE %I', binding.login_name);
                    EXCEPTION WHEN dependent_objects_still_exist THEN
                        -- A catalog dependency must not delay any connection drain.
                        -- The already-disabled role can be cleaned up by a DBA.
                        RAISE WARNING 'Disabled manager login % still owns objects', binding.login_name;
                    END;
                END IF;
                UPDATE manager_db_credential SET drained_at = clock_timestamp()
                WHERE manager_session = binding.manager_session;
                drained_count := drained_count + 1;
                CONTINUE;
            END IF;
            UPDATE manager_db_credential SET drain_retry_after = clock_timestamp() + interval '2 seconds'
            WHERE manager_session = binding.manager_session;
        EXCEPTION WHEN OTHERS THEN
            UPDATE manager_db_credential SET drain_retry_after = clock_timestamp() + interval '15 seconds'
            WHERE manager_session = binding.manager_session;
            RAISE WARNING 'Manager database drain failed for session % (SQLSTATE %); will retry',
                binding.manager_session, SQLSTATE;
        END;
    END LOOP;
    RETURN drained_count;
END
$body$;


CREATE FUNCTION claim_manager_db_pod_checks(batch_size INTEGER)
RETURNS TABLE (session_id BIGINT, pod_uid UUID, pod_name TEXT, manager_role worker_role_enum, pool TEXT)
LANGUAGE sql VOLATILE STRICT SECURITY DEFINER
AS $body$
    WITH due AS (
        SELECT manager_session FROM manager_db_credential
        WHERE drained_at IS NULL AND retired_at IS NULL AND NOT pod_invalidated
          AND pod_check_after <= statement_timestamp()
        ORDER BY pod_check_after, manager_session
        LIMIT greatest(0, least(batch_size, 16))
        FOR UPDATE SKIP LOCKED
    ), claimed AS (
        UPDATE manager_db_credential c SET pod_check_after = clock_timestamp() + interval '20 seconds'
        FROM due WHERE c.manager_session = due.manager_session
        RETURNING c.manager_session, c.pod_uid
    )
    SELECT c.manager_session, c.pod_uid, s.name, s.role, s.pool
    FROM claimed c JOIN manager_session s ON s.id = c.manager_session
$body$;

CREATE FUNCTION invalidate_manager_db_pod(session_id BIGINT, expected_pod UUID)
RETURNS VOID
LANGUAGE sql VOLATILE STRICT SECURITY DEFINER
AS $body$
    UPDATE manager_db_credential SET pod_invalidated = true, pending_verifier = NULL
    WHERE manager_session = session_id AND pod_uid = expected_pod AND retired_at IS NULL
$body$;

REVOKE ALL ON FUNCTION valid_manager_db_role_privileges(OID, worker_role_enum),
    claim_manager_db_pod_checks(INTEGER), invalidate_manager_db_pod(BIGINT, UUID),
    install_requested_manager_db_logins(INTEGER),
    disable_retired_manager_db_logins(INTEGER), drain_retired_manager_db_connections(INTEGER) FROM PUBLIC;

DO $trust$
DECLARE
    signature TEXT;
BEGIN
    FOREACH signature IN ARRAY ARRAY[
        'valid_manager_db_role_privileges(oid,worker_role_enum)',
        'claim_manager_db_pod_checks(integer)',
        'invalidate_manager_db_pod(bigint,uuid)',
        'install_requested_manager_db_logins(integer)',
        'disable_retired_manager_db_logins(integer)',
        'drain_retired_manager_db_connections(integer)'
    ] LOOP
        EXECUTE format('ALTER FUNCTION %s SET search_path TO pg_catalog, %I, pg_temp',
            signature, current_schema());
    END LOOP;
END
$trust$;
