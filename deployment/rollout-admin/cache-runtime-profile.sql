-- Explicit offline DBA supplement for cache-capable development runtime roles.
-- Existing externally owned names+OIDs only; no role/schema/ownership creation.
DO $profile$
DECLARE d JSONB; s NAME; e JSONB; r RECORD; owner_id OID; cache_id OID; api_id OID; member_id OID;
 owner_name NAME; cache_name NAME; api_name NAME; maintenance_name NAME;
 tables TEXT[]:=ARRAY['cache_namespace','cache_generation','cache_entry','cache_ingest_chunk','cache_generation_clone','cache_generation_entry_usage','cache_owner_physical_byte_usage','cache_deployment_physical_byte_usage','cache_entry_statistics_state','cache_live_request'];
 t TEXT; creator OID;
BEGIN
 SELECT document INTO STRICT d FROM pg_temp.cache_runtime_provision_input;
 IF (SELECT array_agg(k ORDER BY k) FROM jsonb_object_keys(d) k) IS DISTINCT FROM ARRAY['api','cache_owner','database_oid','installation','maintenance','migrator','owner','schema','schema_oid','version']::TEXT[] OR (d->>'version') IS DISTINCT FROM '1' THEN RAISE EXCEPTION 'Invalid cache role inventory'; END IF;
 s:=(d->>'schema')::NAME;
 IF s::TEXT<>d->>'schema' OR s::TEXT !~ '^[a-z][a-z0-9_]{0,62}$' THEN RAISE EXCEPTION 'Invalid schema'; END IF;
 FOR e IN SELECT value FROM jsonb_array_elements(jsonb_build_array(d->'owner',d->'cache_owner',d->'api',d->'migrator') || CASE WHEN d->'maintenance'='null'::JSONB THEN '[]'::JSONB ELSE jsonb_build_array(d->'maintenance') END) LOOP
  IF (SELECT count(*) FROM jsonb_object_keys(e))<>2 OR e->>'name' !~ '^[a-z][a-z0-9_]{0,62}$' THEN RAISE EXCEPTION 'Invalid owned role'; END IF;
  SELECT * INTO STRICT r FROM pg_roles WHERE oid=(e->>'oid')::OID AND rolname=e->>'name';
  IF r.rolsuper OR r.rolcreaterole OR r.rolcreatedb OR r.rolreplication OR r.rolbypassrls OR r.rolcanlogin IS DISTINCT FROM (e NOT IN (d->'owner',d->'cache_owner')) THEN RAISE EXCEPTION 'Conflicting role attributes'; END IF;
 END LOOP;
 IF (SELECT count(DISTINCT (x->>'oid')::OID) FROM jsonb_array_elements(jsonb_build_array(d->'owner',d->'cache_owner',d->'api',d->'migrator') || CASE WHEN d->'maintenance'='null'::JSONB THEN '[]'::JSONB ELSE jsonb_build_array(d->'maintenance') END) x) <> (CASE WHEN d->'maintenance'='null'::JSONB THEN 4 ELSE 5 END) THEN RAISE EXCEPTION 'Roles must be distinct'; END IF;
 owner_id:=(d->'owner'->>'oid')::OID;cache_id:=(d->'cache_owner'->>'oid')::OID;api_id:=(d->'api'->>'oid')::OID;
 owner_name:=(d->'owner'->>'name')::NAME;cache_name:=(d->'cache_owner'->>'name')::NAME;api_name:=(d->'api'->>'name')::NAME;
 IF NOT EXISTS(SELECT 1 FROM pg_database WHERE datname=current_database() AND oid=(d->>'database_oid')::OID) OR NOT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=s AND oid=(d->>'schema_oid')::OID AND nspowner=owner_id) OR EXISTS(SELECT 1 FROM pg_namespace n CROSS JOIN LATERAL aclexplode(COALESCE(n.nspacl,acldefault('n',n.nspowner))) a WHERE n.nspname IN(s,'pg_catalog') AND a.privilege_type='CREATE' AND a.grantee<>n.nspowner) THEN RAISE EXCEPTION 'Trusted schema ownership/ACL required'; END IF;
 IF EXISTS(SELECT 1 FROM pg_auth_members WHERE member=cache_id) OR EXISTS(SELECT 1 FROM pg_auth_members WHERE member=owner_id AND (roleid<>cache_id OR NOT inherit_option OR NOT set_option OR admin_option)) OR EXISTS(SELECT 1 FROM pg_auth_members WHERE member=api_id) THEN RAISE EXCEPTION 'Unexpected owner/runtime membership'; END IF;
 member_id:=(d->'migrator'->>'oid')::OID;
 IF NOT EXISTS(SELECT 1 FROM pg_auth_members WHERE member=member_id AND roleid=owner_id AND NOT inherit_option AND set_option AND NOT admin_option) OR EXISTS(SELECT 1 FROM pg_auth_members WHERE member=member_id AND (roleid<>owner_id OR inherit_option OR NOT set_option OR admin_option)) THEN RAISE EXCEPTION 'Exact migrator assumption required'; END IF;
 IF has_schema_privilege(api_id,s,'CREATE') OR has_schema_privilege(api_id,'pg_catalog','CREATE') OR has_schema_privilege(api_id,'public','CREATE') OR has_database_privilege(api_id,current_database(),'CREATE') THEN RAISE EXCEPTION 'API may not create schema objects'; END IF;
 IF EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace CROSS JOIN LATERAL aclexplode(COALESCE(c.relacl,acldefault('r',c.relowner))) a WHERE n.nspname=s AND c.relname=ANY(tables) AND a.grantee=api_id AND (a.is_grantable OR a.privilege_type NOT IN ('SELECT','INSERT','UPDATE','DELETE')))
 OR has_table_privilege(api_id,to_regclass(format('%I.cache_partition_creation_intent',s)),'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') THEN RAISE EXCEPTION 'Unexpected cache runtime privilege'; END IF;
 creator:=to_regprocedure(format('%I.create_cache_generation_partition(bigint)',s));
 IF NOT EXISTS(SELECT 1 FROM pg_proc p WHERE p.oid=creator AND p.proowner=owner_id AND p.prosecdef AND p.proconfig=ARRAY[format('search_path=pg_catalog, %I, pg_temp',s)])
 OR EXISTS(SELECT 1 FROM pg_proc p CROSS JOIN LATERAL aclexplode(COALESCE(p.proacl,acldefault('f',p.proowner))) a WHERE p.oid=creator AND (a.grantee NOT IN(owner_id,api_id) OR (a.grantee=api_id AND a.is_grantable))) THEN RAISE EXCEPTION 'Guarded candidate and exact creator ACL required'; END IF;
 IF NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_partitioned_table p ON p.partrelid=c.oid WHERE n.nspname=s AND c.relname='cache_entry' AND c.relowner=cache_id AND p.partstrat='l') OR EXISTS(SELECT 1 FROM pg_inherits h JOIN pg_class c ON c.oid=h.inhrelid WHERE h.inhparent=to_regclass(format('%I.cache_entry',s)) AND c.relowner<>cache_id) THEN RAISE EXCEPTION 'Existing cache ownership conflicts; no ownership adoption'; END IF;
 IF NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=s AND c.relname='_sqlx_migrations' AND c.relowner=owner_id AND c.relkind='r') THEN RAISE EXCEPTION 'Trusted SQLx history required'; END IF;
 EXECUTE format('SELECT 1 FROM %I.recorded_rollout_active WHERE singleton FOR UPDATE',s) INTO STRICT r;
 EXECUTE format('SELECT installation FROM %I.recorded_rollout_installation_identity WHERE id=1',s) INTO STRICT r;
 IF r.installation IS DISTINCT FROM (d->>'installation')::UUID THEN RAISE EXCEPTION 'Installation mismatch'; END IF;
 EXECUTE format('GRANT %I TO %I WITH INHERIT TRUE, SET TRUE',cache_name,owner_name);
 EXECUTE format('GRANT USAGE ON SCHEMA %I TO %I,%I',s,api_name,cache_name);
 FOREACH t IN ARRAY tables LOOP EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON %I.%I TO %I',s,t,api_name); END LOOP;
 FOREACH t IN ARRAY ARRAY['runtime_retention_config','platform_catalog_state','file_evidence_pin','runtime_native_cache_frontier','workflow_cache_iteration','workflow_execution'] LOOP EXECUTE format('GRANT SELECT ON %I.%I TO %I',s,t,api_name); END LOOP;
 EXECUTE format('GRANT USAGE,SELECT ON SEQUENCE %I.cache_namespace_id_seq,%I.cache_generation_id_seq,%I.cache_entry_id_seq,%I.cache_ingest_chunk_id_seq TO %I',s,s,s,s,api_name);
 EXECUTE format('GRANT EXECUTE ON FUNCTION %I.create_cache_generation_partition(BIGINT) TO %I',s,api_name);
 IF d->'maintenance'<>'null'::JSONB THEN
  member_id:=(d->'maintenance'->>'oid')::OID;maintenance_name:=(d->'maintenance'->>'name')::NAME;
  IF EXISTS(SELECT 1 FROM pg_auth_members WHERE member=member_id AND (roleid<>cache_id OR NOT inherit_option OR set_option OR admin_option)) OR has_schema_privilege(member_id,s,'CREATE') OR has_schema_privilege(member_id,'public','CREATE') OR has_schema_privilege(member_id,'pg_catalog','CREATE') OR has_database_privilege(member_id,current_database(),'CREATE') THEN RAISE EXCEPTION 'Maintenance scope conflict'; END IF;
  EXECUTE format('GRANT %I TO %I WITH INHERIT TRUE, SET FALSE',cache_name,maintenance_name);
  EXECUTE format('GRANT USAGE ON SCHEMA %I TO %I',s,maintenance_name);
  FOREACH t IN ARRAY tables LOOP EXECUTE format('GRANT SELECT,UPDATE,DELETE ON %I.%I TO %I',s,t,maintenance_name); END LOOP;
  FOREACH t IN ARRAY ARRAY['file_evidence_pin','runtime_native_cache_frontier','workflow_cache_iteration','workflow_execution','runtime_retention_config'] LOOP EXECUTE format('GRANT SELECT ON %I.%I TO %I',s,t,maintenance_name); END LOOP;
 END IF;
END $profile$;
