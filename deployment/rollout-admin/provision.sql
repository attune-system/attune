-- DBA-only, transaction-owned input: pg_temp.rollout_provision_input(document JSONB).
-- No roles, schemas, ownership, passwords, application DML or defaults are created.
DO $provision$
DECLARE d JSONB; s NAME; entry JSONB; r RECORD; role_name NAME; owner_id OID;
 cap NAME; op NAME; old_op NAME; old_id OID; installation UUID; names TEXT[] := '{}';
 tables TEXT[] := ARRAY['recorded_rollout_generation','recorded_rollout_active','recorded_rollout_installation_identity','recorded_rollout_offline_receipt','recorded_rollout_receipt_terminal'];
 fn TEXT; t TEXT;
BEGIN
 SELECT document INTO STRICT d FROM pg_temp.rollout_provision_input;
 IF (SELECT array_agg(k ORDER BY k) FROM jsonb_object_keys(d) k) IS DISTINCT FROM ARRAY['cache_owner','capability','installation','migrator','operation','operator','owner','previous_operator','runtimes','schema','version']::TEXT[] OR d->>'version'<>'1' OR d->>'operation' NOT IN ('configure','rotate')
 OR jsonb_array_length(d->'runtimes') NOT BETWEEN 1 AND 64 THEN RAISE EXCEPTION 'Invalid provisioning inventory'; END IF;
 s:=(d->>'schema')::NAME;
 IF s::TEXT<>d->>'schema' OR s::TEXT !~ '^[a-z][a-z0-9_]{0,62}$' THEN RAISE EXCEPTION 'Invalid installation schema'; END IF;
 -- Exact names AND cluster OIDs are externally owned IaC state, never discovered/adopted here.
 FOR entry IN SELECT value FROM jsonb_array_elements(jsonb_build_array(d->'owner',d->'migrator',d->'cache_owner',d->'operator',d->'capability') || d->'runtimes') LOOP
  IF (SELECT count(*) FROM jsonb_object_keys(entry))<>2 OR entry->>'name' !~ '^[a-z][a-z0-9_]{0,62}$' OR (entry->>'name')=ANY(names) THEN RAISE EXCEPTION 'Invalid or duplicate owned role'; END IF;
  SELECT * INTO STRICT r FROM pg_catalog.pg_roles WHERE rolname=entry->>'name' AND oid=(entry->>'oid')::OID;
  IF r.rolsuper OR r.rolcreaterole OR r.rolcreatedb OR r.rolreplication OR r.rolbypassrls THEN RAISE EXCEPTION 'Conflicting role attributes'; END IF;
  IF r.rolcanlogin IS DISTINCT FROM (entry NOT IN (d->'owner',d->'cache_owner',d->'capability')) THEN RAISE EXCEPTION 'Conflicting login posture'; END IF;
  names:=array_append(names,r.rolname::TEXT);
 END LOOP;
 owner_id:=(d->'owner'->>'oid')::OID; cap:=(d->'capability'->>'name')::NAME; op:=(d->'operator'->>'name')::NAME;
 IF NOT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=s AND nspowner=owner_id) OR EXISTS(SELECT 1 FROM pg_namespace n CROSS JOIN LATERAL aclexplode(COALESCE(n.nspacl,acldefault('n',n.nspowner))) a WHERE n.nspname IN (s,'pg_catalog') AND a.privilege_type='CREATE' AND a.grantee<>n.nspowner) THEN RAISE EXCEPTION 'Untrusted installation schema'; END IF;
 -- Serialize all configure/rotation with the existing immutable-installation pointer lock.
 EXECUTE format('SELECT singleton FROM %I.recorded_rollout_active WHERE singleton FOR UPDATE',s) INTO STRICT r;
 IF EXISTS(SELECT 1 FROM pg_auth_members WHERE member IN ((d->'owner'->>'oid')::OID,(d->'cache_owner'->>'oid')::OID,(d->'capability'->>'oid')::OID)) THEN RAISE EXCEPTION 'Owner/capability inheritance forbidden'; END IF;
 IF NOT EXISTS(SELECT 1 FROM pg_auth_members WHERE member=(d->'migrator'->>'oid')::OID AND roleid=owner_id AND NOT admin_option AND NOT inherit_option AND set_option)
 OR EXISTS(SELECT 1 FROM pg_auth_members WHERE member=(d->'migrator'->>'oid')::OID AND roleid<>owner_id) THEN RAISE EXCEPTION 'Exact offline migrator SET membership required'; END IF;
 FOR entry IN SELECT value FROM jsonb_array_elements(d->'runtimes') LOOP
  role_name:=(entry->>'name')::NAME;
  IF EXISTS(SELECT 1 FROM pg_auth_members WHERE member=(entry->>'oid')::OID) OR has_schema_privilege(role_name,s,'CREATE') OR has_database_privilege(role_name,current_database(),'CREATE') THEN RAISE EXCEPTION 'Runtime DDL/role posture conflicts'; END IF;
  FOREACH t IN ARRAY tables LOOP
   IF has_table_privilege(role_name,format('%I.%I',s,t),'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') OR has_any_column_privilege(role_name,format('%I.%I',s,t),'INSERT,UPDATE,REFERENCES') OR (t IN ('recorded_rollout_offline_receipt','recorded_rollout_receipt_terminal') AND has_any_column_privilege(role_name,format('%I.%I',s,t),'SELECT')) THEN RAISE EXCEPTION 'Runtime rollout privileges forbidden'; END IF;
  END LOOP;
  FOR fn IN SELECT p.oid::regprocedure::TEXT FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=s AND p.proname IN ('stage_recorded_rollout','activate_recorded_rollout','issue_recorded_rollout_receipt','consume_recorded_rollout_receipt','revoke_recorded_rollout_receipt','recorded_rollout_admin_ledger') LOOP
   IF has_function_privilege(role_name,fn,'EXECUTE') THEN RAISE EXCEPTION 'Runtime mutation execute forbidden'; END IF;
  END LOOP;
 END LOOP;
 IF NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=s AND c.relname='_sqlx_migrations' AND c.relkind='r' AND c.relowner=owner_id AND NOT c.relrowsecurity AND NOT c.relforcerowsecurity) THEN RAISE EXCEPTION 'Trusted SQLx ledger required'; END IF;
 -- Prove helper provenance BEFORE executing it. Do not transfer object ownership.
 IF (SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=s AND p.proname IN ('recorded_rollout_installation','recorded_rollout_operator_role') AND p.proowner=owner_id AND p.proconfig=ARRAY[format('search_path=pg_catalog, %I, pg_temp',s)])<>2 THEN RAISE EXCEPTION 'Untrusted installation helpers'; END IF;
 EXECUTE format('SELECT %I.recorded_rollout_operator_role()',s) INTO role_name;
 EXECUTE format('SELECT (%I.recorded_rollout_installation()->>''uuid'')::UUID',s) INTO installation;
 IF role_name<>cap OR installation IS DISTINCT FROM (d->>'installation')::UUID THEN RAISE EXCEPTION 'Wrong installation/capability'; END IF;
 -- Reject unrelated table/column, sequence and explicit function grants; do not repair.
 IF EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=s AND c.relkind IN ('r','p','v','m','f') AND (has_table_privilege(op,c.oid,'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') OR has_any_column_privilege(op,c.oid,'INSERT,UPDATE,REFERENCES') OR (c.relname::TEXT<>ALL(tables) AND has_any_column_privilege(op,c.oid,'SELECT')) OR EXISTS(SELECT 1 FROM aclexplode(COALESCE(c.relacl,acldefault('r',c.relowner))) a WHERE a.grantee IN ((d->'operator'->>'oid')::OID,(d->'capability'->>'oid')::OID,0) AND (a.is_grantable OR a.privilege_type<>'SELECT' OR c.relname::TEXT<>ALL(tables)))))
 OR EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=s AND CASE WHEN c.relkind='S' THEN has_sequence_privilege(op,c.oid,'USAGE,SELECT,UPDATE') ELSE FALSE END)
 OR EXISTS(SELECT 1 FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace CROSS JOIN LATERAL aclexplode(COALESCE(p.proacl,acldefault('f',p.proowner))) a WHERE n.nspname=s AND a.grantee IN ((d->'operator'->>'oid')::OID,(d->'capability'->>'oid')::OID) AND (a.is_grantable OR p.proname NOT IN ('issue_recorded_rollout_receipt','consume_recorded_rollout_receipt','revoke_recorded_rollout_receipt','recorded_rollout_admin_ledger'))) THEN RAISE EXCEPTION 'Unexpected offline privileges'; END IF;
 -- Never repair extra memberships or grant options on an existing operator.
 IF EXISTS(SELECT 1 FROM pg_auth_members WHERE member=(d->'operator'->>'oid')::OID AND (roleid<>(d->'capability'->>'oid')::OID OR admin_option OR inherit_option OR set_option)) OR has_schema_privilege(op,s,'CREATE') OR has_database_privilege(op,current_database(),'CREATE') THEN RAISE EXCEPTION 'Conflicting operator posture'; END IF;
 IF d->>'operation'='rotate' THEN
  old_op:=(d->'previous_operator'->>'name')::NAME; old_id:=(d->'previous_operator'->>'oid')::OID;
  IF old_op=op OR NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname=old_op AND oid=old_id AND rolcanlogin AND NOT rolsuper AND NOT rolcreaterole AND NOT rolcreatedb AND NOT rolreplication AND NOT rolbypassrls) OR EXISTS(SELECT 1 FROM pg_auth_members WHERE member=old_id AND (roleid<>(d->'capability'->>'oid')::OID OR admin_option OR inherit_option OR set_option)) OR NOT EXISTS(SELECT 1 FROM pg_auth_members WHERE member=old_id AND roleid=(d->'capability'->>'oid')::OID AND NOT admin_option AND NOT inherit_option AND NOT set_option) OR EXISTS(SELECT 1 FROM pg_stat_activity WHERE usesysid=old_id) THEN RAISE EXCEPTION 'Old operator must be exact, quiesced and authorized'; END IF;
  EXECUTE format('REVOKE %I FROM %I',cap,old_op);
  EXECUTE format('REVOKE USAGE ON SCHEMA %I FROM %I',s,old_op);
  FOREACH t IN ARRAY tables LOOP EXECUTE format('REVOKE SELECT ON %I.%I FROM %I',s,t,old_op); END LOOP;
 ELSIF d->'previous_operator'<>'null'::JSONB THEN RAISE EXCEPTION 'Unexpected previous operator'; END IF;
 EXECUTE format('GRANT %I TO %I WITH INHERIT FALSE, SET FALSE',cap,op);
 EXECUTE format('GRANT USAGE ON SCHEMA %I TO %I',s,op);
 FOREACH t IN ARRAY tables LOOP EXECUTE format('GRANT SELECT ON %I.%I TO %I',s,t,op); END LOOP;
 FOREACH fn IN ARRAY ARRAY['issue_recorded_rollout_receipt(jsonb)','consume_recorded_rollout_receipt(jsonb)','revoke_recorded_rollout_receipt(jsonb)','recorded_rollout_admin_ledger()'] LOOP
  IF old_op IS NOT NULL THEN EXECUTE format('REVOKE EXECUTE ON FUNCTION %I.%s FROM %I',s,fn,old_op); END IF;
  EXECUTE format('GRANT EXECUTE ON FUNCTION %I.%s TO %I',s,fn,op);
 END LOOP;
END $provision$;
