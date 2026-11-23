-- Run as the DBA with psql variables app_schema, login_name, manager_role,
-- manager_pool, manager_slot and manager_instance. Bind only a verified boot.
\set ON_ERROR_STOP on
BEGIN;
SET LOCAL search_path TO :"app_schema", pg_temp;
SELECT set_config('attune.manager_db_login', :'login_name', true),
       set_config('attune.manager_db_role', :'manager_role', true),
       set_config('attune.manager_db_pool', :'manager_pool', true),
       set_config('attune.manager_db_slot', :'manager_slot', true),
       set_config('attune.manager_db_instance', :'manager_instance', true);

DO $bind$
DECLARE
    requested_role worker_role_enum := current_setting('attune.manager_db_role')::worker_role_enum;
    boot UUID := current_setting('attune.manager_db_instance')::UUID;
    target BIGINT;
    login_oid OID;
    login NAME := current_setting('attune.manager_db_login')::NAME;
BEGIN
    SELECT s.id INTO STRICT target FROM manager_session s
    WHERE s.role = requested_role AND s.pool = current_setting('attune.manager_db_pool')
        AND s.name = current_setting('attune.manager_db_slot') AND s.instance = boot
        AND s.closed_at IS NULL AND s.expires_at > clock_timestamp()
    FOR SHARE;

    SELECT r.oid INTO STRICT login_oid FROM pg_roles r
    WHERE r.rolname = login AND r.rolcanlogin AND NOT r.rolsuper AND NOT r.rolcreatedb
        AND NOT r.rolcreaterole AND NOT r.rolreplication AND NOT r.rolbypassrls
        AND (SELECT count(*) FROM pg_auth_members m WHERE m.member = r.oid) = 1
        AND NOT EXISTS (
            SELECT 1 FROM pg_auth_members m
            WHERE (m.member = r.oid AND (m.admin_option OR m.set_option OR NOT m.inherit_option))
                OR m.roleid = r.oid
        )
        AND NOT has_schema_privilege(r.oid, current_schema(), 'CREATE')
        AND NOT EXISTS (
            SELECT 1 FROM pg_class relation
            JOIN pg_namespace namespace ON namespace.oid = relation.relnamespace
            WHERE namespace.nspname = current_schema()
                AND CASE
                    WHEN relation.relkind = 'S' THEN
                        has_sequence_privilege(r.oid, relation.oid, 'USAGE, SELECT, UPDATE')
                    WHEN relation.relkind IN ('r', 'p', 'v', 'm', 'f') THEN
                        has_any_column_privilege(r.oid, relation.oid, 'SELECT, INSERT, UPDATE, REFERENCES')
                        OR has_table_privilege(r.oid, relation.oid, 'DELETE, TRUNCATE, TRIGGER')
                    ELSE FALSE
                END
        )
        AND has_function_privilege(r.oid, 'manager_db_action_session(uuid,boolean)', 'EXECUTE')
            = (requested_role = 'action')
        AND has_function_privilege(r.oid, 'manager_db_sensor_session(uuid,boolean)', 'EXECUTE')
            = (requested_role = 'sensor')
        AND NOT has_function_privilege(
            r.oid, 'inspect_manager_db_session(worker_role_enum,uuid,boolean)', 'EXECUTE');

    INSERT INTO manager_db_principal (principal, login_name, manager_session, instance)
    VALUES (login_oid, login, target, boot)
    ON CONFLICT DO NOTHING;

    IF NOT EXISTS (
        SELECT 1 FROM manager_db_principal b
        JOIN manager_session s ON s.id = b.manager_session
        WHERE b.principal = login_oid AND b.login_name = login AND b.manager_session = target
            AND b.instance = boot AND b.revoked_at IS NULL AND s.expires_at > clock_timestamp()
    ) THEN
        RAISE EXCEPTION 'Conflicting, revoked or expired manager database binding';
    END IF;
END
$bind$;
COMMIT;
