-- Read-only DBA inventory, deliberately no GRANTs. Capture exact live ACLs,
-- membership options, owner identities and executable DDL definitions for review.
SELECT rolname, oid, rolcanlogin, rolsuper, rolcreaterole, rolcreatedb, rolreplication, rolbypassrls
FROM pg_catalog.pg_roles ORDER BY rolname;
SELECT roleid, member, grantor, admin_option, inherit_option, set_option
FROM pg_catalog.pg_auth_members ORDER BY roleid,member,grantor;
SELECT n.nspname, n.nspowner, n.nspacl, c.relname, c.relkind, c.relowner, c.relacl
FROM pg_catalog.pg_namespace n JOIN pg_catalog.pg_class c ON c.relnamespace=n.oid
WHERE n.nspname NOT LIKE 'pg_%' AND n.nspname<>'information_schema' ORDER BY n.nspname,c.relname;
SELECT n.nspname,c.relname,a.attname,a.attacl
FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
WHERE a.attacl IS NOT NULL AND n.nspname NOT LIKE 'pg_%' ORDER BY n.nspname,c.relname,a.attnum;
SELECT n.nspname,p.proname,p.oid,p.proowner,p.prosecdef,p.proconfig,p.proacl,pg_get_functiondef(p.oid) AS definition
FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace
WHERE n.nspname NOT LIKE 'pg_%' AND n.nspname<>'information_schema' AND p.prokind='f'
ORDER BY n.nspname,p.proname,p.oid;
