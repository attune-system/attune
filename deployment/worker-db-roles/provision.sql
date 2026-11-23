-- Run as the DBA after migration. Required psql variables identify the
-- application schema and three dedicated NOLOGIN roles in this cluster.
\set ON_ERROR_STOP on
BEGIN;
SET LOCAL search_path TO :"app_schema", pg_temp;
SELECT set_config('attune.manager_db_owner', :'operation_owner', true),
       set_config('attune.manager_db_action', :'action_group', true),
       set_config('attune.manager_db_sensor', :'sensor_group', true);

DO $verify$
DECLARE
    expected NAME[] := ARRAY[
        current_setting('attune.manager_db_owner')::NAME,
        current_setting('attune.manager_db_action')::NAME,
        current_setting('attune.manager_db_sensor')::NAME
    ];
BEGIN
    IF (SELECT count(*) FROM pg_roles WHERE rolname = ANY(expected)) <> 3 OR EXISTS (
        SELECT 1 FROM pg_roles r WHERE r.rolname = ANY(expected)
        AND (r.rolcanlogin OR r.rolsuper OR r.rolcreaterole OR r.rolcreatedb
             OR r.rolreplication OR r.rolbypassrls
             OR EXISTS (SELECT 1 FROM pg_auth_members m WHERE m.member = r.oid))
    ) OR EXISTS (
        SELECT 1 FROM pg_auth_members m JOIN pg_roles r ON r.oid = m.roleid
        WHERE r.rolname = expected[1]
    ) THEN
        RAISE EXCEPTION 'Expected three distinct restricted roles and an unassumable operation owner';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_namespace n
        CROSS JOIN LATERAL aclexplode(COALESCE(n.nspacl, acldefault('n', n.nspowner))) acl
        WHERE n.nspname IN (current_schema(), 'pg_catalog')
          AND acl.privilege_type = 'CREATE' AND acl.grantee <> n.nspowner
    ) THEN
        RAISE EXCEPTION 'Manager operations require trusted schema creation privileges';
    END IF;
END
$verify$;

GRANT USAGE ON SCHEMA :"app_schema" TO :"operation_owner", :"action_group", :"sensor_group";
GRANT SELECT (principal, login_name, manager_session, instance, revoked_at),
      UPDATE (revoked_at) ON manager_db_principal TO :"operation_owner";
GRANT SELECT (id, role, instance, expires_at, closed_at),
      UPDATE (closed_at) ON manager_session TO :"operation_owner";
GRANT EXECUTE ON FUNCTION inspect_manager_db_session(worker_role_enum, UUID, BOOLEAN)
    TO :"operation_owner";

ALTER FUNCTION manager_db_action_session(UUID, BOOLEAN) OWNER TO :"operation_owner";
ALTER FUNCTION manager_db_sensor_session(UUID, BOOLEAN) OWNER TO :"operation_owner";
GRANT EXECUTE ON FUNCTION manager_db_action_session(UUID, BOOLEAN) TO :"action_group";
GRANT EXECUTE ON FUNCTION manager_db_sensor_session(UUID, BOOLEAN) TO :"sensor_group";
COMMIT;
