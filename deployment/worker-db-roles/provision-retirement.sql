-- Run after provision.sql. The supervisor connects as the restricted caller.
-- The dedicated NOLOGIN owner administers only the managed role groups and
-- the login roles it creates for this installation.
\set ON_ERROR_STOP on
BEGIN;
SET LOCAL search_path TO :"app_schema", pg_temp;
SELECT set_config('attune.manager_db_retirement_owner', :'retirement_owner', true),
       set_config('attune.manager_db_retirement_caller', :'retirement_caller', true),
       set_config('attune.manager_db_api', :'api_role', true),
       set_config('attune.manager_db_action', :'action_group', true),
       set_config('attune.manager_db_sensor', :'sensor_group', true);

DO $verify$
BEGIN
    IF (SELECT count(*) FROM pg_roles WHERE rolname IN (
        current_setting('attune.manager_db_retirement_owner'),
        current_setting('attune.manager_db_retirement_caller'),
        current_setting('attune.manager_db_api'),
        current_setting('attune.manager_db_action'),
        current_setting('attune.manager_db_sensor')
    )) <> 5 OR NOT EXISTS (
        SELECT 1 FROM pg_roles r
        WHERE r.rolname = current_setting('attune.manager_db_retirement_owner')
          AND r.rolcreaterole AND NOT (r.rolcanlogin OR r.rolsuper OR r.rolcreatedb
                                      OR r.rolreplication OR r.rolbypassrls)
          AND NOT EXISTS (SELECT 1 FROM pg_auth_members m WHERE m.roleid = r.oid)
    ) OR NOT EXISTS (
        SELECT 1 FROM pg_roles r
        WHERE r.rolname = current_setting('attune.manager_db_retirement_caller')
          AND r.rolcanlogin AND NOT (r.rolsuper OR r.rolcreaterole OR r.rolcreatedb
                                    OR r.rolreplication OR r.rolbypassrls)
          AND NOT EXISTS (SELECT 1 FROM pg_auth_members m WHERE m.member = r.oid)
    ) OR EXISTS (
        SELECT 1 FROM pg_namespace n
        CROSS JOIN LATERAL aclexplode(coalesce(n.nspacl, acldefault('n', n.nspowner))) acl
        WHERE n.nspname IN (current_schema(), 'pg_catalog')
          AND acl.privilege_type = 'CREATE' AND acl.grantee <> n.nspowner
    ) THEN
        RAISE EXCEPTION 'Expected dedicated manager retirement roles and trusted schemas';
    END IF;
END
$verify$;

INSERT INTO manager_db_provisioning (singleton, login_prefix, action_group, sensor_group)
VALUES (TRUE, :'login_prefix', :'action_group', :'sensor_group')
ON CONFLICT (singleton) DO UPDATE SET login_prefix = EXCLUDED.login_prefix
WHERE manager_db_provisioning.login_prefix = EXCLUDED.login_prefix
  AND manager_db_provisioning.action_group = EXCLUDED.action_group
  AND manager_db_provisioning.sensor_group = EXCLUDED.sensor_group
RETURNING singleton \gset

GRANT :"action_group", :"sensor_group" TO :"retirement_owner"
    WITH ADMIN TRUE, INHERIT FALSE, SET FALSE;
GRANT CONNECT ON DATABASE :"database" TO :"action_group", :"sensor_group", :"retirement_caller";
GRANT USAGE ON SCHEMA :"app_schema" TO :"retirement_owner", :"retirement_caller", :"api_role";
GRANT SELECT ON manager_db_provisioning TO :"retirement_owner";
GRANT SELECT, UPDATE ON manager_db_credential TO :"retirement_owner";
GRANT SELECT, INSERT, UPDATE ON manager_db_credential TO :"api_role";
GRANT INSERT (principal, login_name, manager_session, instance),
      SELECT (principal, revoked_at), UPDATE (revoked_at)
    ON manager_db_principal TO :"retirement_owner";
GRANT SELECT (id, instance, role, name, pool, closed_at, expires_at), UPDATE (closed_at)
    ON manager_session TO :"retirement_owner";

ALTER FUNCTION claim_manager_db_pod_checks(INTEGER) OWNER TO :"retirement_owner";
ALTER FUNCTION invalidate_manager_db_pod(BIGINT, UUID) OWNER TO :"retirement_owner";
ALTER FUNCTION install_requested_manager_db_logins(INTEGER) OWNER TO :"retirement_owner";
ALTER FUNCTION disable_retired_manager_db_logins(INTEGER) OWNER TO :"retirement_owner";
ALTER FUNCTION drain_retired_manager_db_connections(INTEGER) OWNER TO :"retirement_owner";
GRANT EXECUTE ON FUNCTION claim_manager_db_pod_checks(INTEGER),
    invalidate_manager_db_pod(BIGINT, UUID), install_requested_manager_db_logins(INTEGER),
    disable_retired_manager_db_logins(INTEGER),
    drain_retired_manager_db_connections(INTEGER) TO :"retirement_caller";
COMMIT;
