-- Candidate for forward migration 9. Test transactionally before first application.
-- Private synchronous DDL intent, not a runtime/API authority grant or work receipt.
CREATE TABLE cache_partition_creation_intent (
 generation BIGINT PRIMARY KEY REFERENCES cache_generation(id) ON DELETE CASCADE,
 installation UUID NOT NULL, namespace BIGINT NOT NULL,
 parent OID NOT NULL, schema_id OID NOT NULL, owner_id OID NOT NULL, cache_owner OID NOT NULL,
 source OID NOT NULL, backend INTEGER NOT NULL, creation_transaction xid8 NOT NULL,
 namespace_mode TEXT NOT NULL, generation_state TEXT NOT NULL,
 consumed BOOLEAN NOT NULL DEFAULT FALSE
);
REVOKE ALL ON cache_partition_creation_intent FROM PUBLIC;

CREATE FUNCTION guard_cache_partition_context() RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE app RECORD; parent_row RECORD; caller OID := session_user::regrole::OID;
BEGIN
 SELECT n.oid,n.nspowner,n.nspacl,p.proowner INTO STRICT app FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace
 WHERE p.oid='create_cache_generation_partition(bigint)'::regprocedure;
 SELECT c.oid,c.relowner,c.relnamespace,c.relkind,pt.partstrat,pt.partnatts,pt.partattrs[0] AS partition_attribute,(SELECT attnum FROM pg_attribute WHERE attrelid=c.oid AND attname='generation' AND NOT attisdropped) AS generation_attribute INTO STRICT parent_row
 FROM pg_class c JOIN pg_partitioned_table pt ON pt.partrelid=c.oid WHERE c.oid='cache_entry'::regclass;
 IF current_user::regrole::OID<>app.nspowner OR app.proowner<>app.nspowner
 OR NOT EXISTS(SELECT 1 FROM pg_roles WHERE oid=app.nspowner AND NOT rolcanlogin AND NOT rolsuper AND NOT rolcreaterole AND NOT rolcreatedb AND NOT rolbypassrls AND NOT rolreplication)
 OR parent_row.relnamespace<>app.oid OR parent_row.relkind<>'p' OR parent_row.partstrat<>'l' OR parent_row.partnatts<>1 OR parent_row.partition_attribute IS DISTINCT FROM parent_row.generation_attribute
 OR NOT EXISTS(SELECT 1 FROM pg_roles WHERE oid=parent_row.relowner AND NOT rolcanlogin AND NOT rolsuper AND NOT rolcreaterole AND NOT rolcreatedb AND NOT rolbypassrls AND NOT rolreplication)
 OR EXISTS(SELECT 1 FROM pg_auth_members WHERE member=parent_row.relowner)
 OR parent_row.relowner=app.nspowner
 OR NOT EXISTS(SELECT 1 FROM pg_auth_members WHERE roleid=parent_row.relowner AND member=app.nspowner AND inherit_option AND set_option AND NOT admin_option)
 OR EXISTS(SELECT 1 FROM pg_auth_members WHERE member=app.nspowner AND (roleid<>parent_row.relowner OR NOT inherit_option OR NOT set_option OR admin_option))
 OR EXISTS(SELECT 1 FROM pg_namespace n CROSS JOIN LATERAL aclexplode(COALESCE(n.nspacl,acldefault('n',n.nspowner))) a WHERE n.oid IN (app.oid,'pg_catalog'::regnamespace) AND a.privilege_type='CREATE' AND a.grantee<>n.nspowner)
 OR NOT EXISTS(SELECT 1 FROM pg_roles WHERE oid=caller AND rolcanlogin AND NOT rolsuper AND NOT rolcreaterole AND NOT rolcreatedb AND NOT rolbypassrls AND NOT rolreplication)
 OR EXISTS(SELECT 1 FROM pg_auth_members WHERE member=caller)
 OR EXISTS(SELECT 1 FROM pg_proc p CROSS JOIN LATERAL aclexplode(p.proacl) a WHERE p.oid='create_cache_generation_partition(bigint)'::regprocedure AND (a.grantee NOT IN (app.nspowner,caller) OR (a.grantee=caller AND a.is_grantable)))
 OR NOT EXISTS(SELECT 1 FROM pg_proc p CROSS JOIN LATERAL aclexplode(p.proacl) a WHERE p.oid='create_cache_generation_partition(bigint)'::regprocedure AND a.grantee=caller AND a.privilege_type='EXECUTE' AND NOT a.is_grantable)
 THEN RAISE EXCEPTION 'Cache partition context denied' USING ERRCODE='42501'; END IF;
 IF NOT EXISTS(SELECT 1 FROM pg_locks WHERE pid=pg_backend_pid() AND locktype='advisory' AND classid=7821101 AND objid=0 AND objsubid=2 AND mode='ExclusiveLock' AND granted)
 OR NOT EXISTS(SELECT 1 FROM pg_locks WHERE pid=pg_backend_pid() AND locktype='relation' AND relation=parent_row.oid AND mode='ShareUpdateExclusiveLock' AND granted)
 THEN RAISE EXCEPTION 'Cache admission and parent protection required' USING ERRCODE='42501'; END IF;
END $$;
REVOKE ALL ON FUNCTION guard_cache_partition_context() FROM PUBLIC;

CREATE FUNCTION admit_cache_partition_creation() RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER AS $$
DECLARE ns cache_namespace; parent_row RECORD; installation_id UUID;
BEGIN
 PERFORM guard_cache_partition_context();
 SELECT * INTO STRICT ns FROM cache_namespace WHERE id=NEW.namespace FOR UPDATE NOWAIT;
 IF ns.tombstoned_at IS NOT NULL OR NOT ((ns.mode='snapshot' AND NEW.state='staging') OR
 (ns.mode='live' AND NEW.state='active' AND NEW.client_refresh_id='live' AND ns.active_generation IS NULL))
 THEN RAISE EXCEPTION 'Cache generation is not admitted for partition creation'; END IF;
 SELECT c.oid,c.relnamespace,c.relowner,n.nspowner INTO STRICT parent_row FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.oid='cache_entry'::regclass;
 SELECT installation INTO STRICT installation_id FROM recorded_rollout_installation_identity WHERE id=1;
 INSERT INTO cache_partition_creation_intent(generation,installation,namespace,parent,schema_id,owner_id,cache_owner,source,backend,creation_transaction,namespace_mode,generation_state)
 VALUES(NEW.id,installation_id,NEW.namespace,parent_row.oid,parent_row.relnamespace,parent_row.nspowner,parent_row.relowner,session_user::regrole::OID,pg_backend_pid(),pg_current_xact_id(),ns.mode::TEXT,NEW.state::TEXT);
 RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION admit_cache_partition_creation() FROM PUBLIC;
CREATE TRIGGER admit_cache_partition_creation AFTER INSERT ON cache_generation FOR EACH ROW EXECUTE FUNCTION admit_cache_partition_creation();

CREATE FUNCTION guard_cache_partition_intent() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF ROW(NEW.generation,NEW.installation,NEW.namespace,NEW.parent,NEW.schema_id,NEW.owner_id,NEW.cache_owner,NEW.source,NEW.backend,NEW.creation_transaction,NEW.namespace_mode,NEW.generation_state)
 IS DISTINCT FROM ROW(OLD.generation,OLD.installation,OLD.namespace,OLD.parent,OLD.schema_id,OLD.owner_id,OLD.cache_owner,OLD.source,OLD.backend,OLD.creation_transaction,OLD.namespace_mode,OLD.generation_state)
 OR OLD.consumed OR NOT NEW.consumed THEN RAISE EXCEPTION 'Immutable one-time cache partition intent'; END IF;
 RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION guard_cache_partition_intent() FROM PUBLIC;
CREATE TRIGGER guard_cache_partition_intent BEFORE UPDATE ON cache_partition_creation_intent FOR EACH ROW EXECUTE FUNCTION guard_cache_partition_intent();

-- Replays validate the same exact ownership invariant, with a trusted path.
CREATE OR REPLACE FUNCTION validate_cache_generation_partition(generation_id BIGINT)
RETURNS TEXT LANGUAGE plpgsql AS $$
DECLARE
    parent_id OID := 'cache_entry'::REGCLASS;
    parent_schema TEXT;
    child_name TEXT := cache_generation_partition_name(generation_id);
    child_id OID;
BEGIN
    SELECT n.nspname INTO STRICT parent_schema
      FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
     WHERE c.oid = parent_id AND c.relkind = 'p';
    SELECT c.oid INTO child_id
      FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
      JOIN pg_inherits i ON i.inhrelid = c.oid AND i.inhparent = parent_id
     WHERE n.nspname = parent_schema AND c.relname = child_name
       AND c.relkind = 'r' AND c.relispartition
       AND c.relowner = (SELECT relowner FROM pg_class WHERE oid=parent_id)
       AND pg_get_expr(c.relpartbound, c.oid) = format('FOR VALUES IN (%L)', generation_id::TEXT);
    IF child_id IS NULL OR NOT EXISTS (
        SELECT 1 FROM cache_generation_entry_usage WHERE generation = generation_id
    ) THEN
        RAISE EXCEPTION 'cache generation partition or usage invariant is invalid';
    END IF;
    RETURN format('%I.%I', parent_schema, child_name);
END;
$$;

CREATE OR REPLACE FUNCTION create_cache_generation_partition(generation_id BIGINT)
RETURNS VOID LANGUAGE plpgsql SECURITY DEFINER AS $$
DECLARE held cache_partition_creation_intent; g cache_generation; ns cache_namespace; parent_row RECORD;
 child_name TEXT; child_relation TEXT; child_id OID; original_acl aclitem[];
BEGIN
 PERFORM guard_cache_partition_context();
 IF generation_id IS NULL OR generation_id<=0 THEN RAISE EXCEPTION 'Positive generation required'; END IF;
 SELECT * INTO STRICT held FROM cache_partition_creation_intent WHERE generation=generation_id FOR UPDATE;
 SELECT * INTO STRICT g FROM cache_generation WHERE id=generation_id;
 SELECT * INTO STRICT ns FROM cache_namespace WHERE id=g.namespace FOR UPDATE NOWAIT;
 SELECT c.oid,c.relowner,c.relnamespace,n.nspowner,n.nspname INTO STRICT parent_row FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.oid='cache_entry'::regclass;
 IF held.creation_transaction<>pg_current_xact_id() OR held.source<>session_user::regrole::OID OR held.backend<>pg_backend_pid()
 OR held.installation IS DISTINCT FROM (SELECT installation FROM recorded_rollout_installation_identity WHERE id=1)
 OR held.parent<>parent_row.oid OR held.schema_id<>parent_row.relnamespace OR held.owner_id<>parent_row.nspowner OR held.cache_owner<>parent_row.relowner
 OR held.namespace<>g.namespace OR held.namespace_mode<>ns.mode::TEXT OR held.generation_state<>g.state::TEXT OR ns.tombstoned_at IS NOT NULL
 THEN RAISE EXCEPTION 'Cache partition intent drift'; END IF;
 child_name:=cache_generation_partition_name(generation_id);
 child_relation:=format('%I.%I',parent_row.nspname,child_name);
 IF held.consumed THEN
  PERFORM validate_cache_generation_partition(generation_id);
  SELECT oid INTO STRICT child_id FROM pg_class WHERE relnamespace=parent_row.relnamespace AND relname=child_name AND relowner=held.cache_owner;
  RETURN;
 END IF;
 IF NOT ((ns.mode='snapshot' AND g.state='staging') OR (ns.mode='live' AND g.state='active' AND ns.active_generation IS NULL AND g.client_refresh_id='live')) THEN RAISE EXCEPTION 'Cache partition state drift'; END IF;
 INSERT INTO cache_owner_physical_byte_usage(owner_type,owner,physical_bytes) VALUES(ns.owner_type,ns.owner,0) ON CONFLICT(owner_type,owner) DO NOTHING;
 INSERT INTO cache_generation_entry_usage(generation) VALUES(generation_id);
 EXECUTE format('CREATE TABLE %s (LIKE cache_entry INCLUDING DEFAULTS INCLUDING CONSTRAINTS)',child_relation);
 EXECUTE format('ALTER TABLE %s ADD CHECK (generation = %s)',child_relation,generation_id);
 EXECUTE format('ALTER TABLE cache_entry ATTACH PARTITION %s FOR VALUES IN (%s)',child_relation,generation_id);
 -- Option A: privilege transition is confined to this statement/transaction.
 -- Any failure/cancel rolls back GRANT, DDL and intent together. Never commit
 -- an ACL change or repair a mismatched starting ACL.
 SELECT nspacl INTO STRICT original_acl FROM pg_namespace WHERE oid=held.schema_id AND nspowner=held.owner_id;
 EXECUTE format('GRANT CREATE ON SCHEMA %I TO %I',parent_row.nspname,pg_get_userbyid(held.cache_owner));
 EXECUTE format('ALTER TABLE %s OWNER TO %I',child_relation,pg_get_userbyid(held.cache_owner));
 EXECUTE format('REVOKE CREATE ON SCHEMA %I FROM %I',parent_row.nspname,pg_get_userbyid(held.cache_owner));
 IF NOT EXISTS(SELECT 1 FROM pg_namespace WHERE oid=held.schema_id AND nspowner=held.owner_id AND nspacl IS NOT DISTINCT FROM original_acl)
 THEN RAISE EXCEPTION 'Cache creation did not restore exact schema identity/ACL'; END IF;
 IF NOT EXISTS(SELECT 1 FROM pg_class WHERE oid=to_regclass(child_relation) AND relowner=held.cache_owner AND relnamespace=held.schema_id)
 THEN RAISE EXCEPTION 'Cache creation owner/namespace drift'; END IF;
 PERFORM validate_cache_generation_partition(generation_id);
 UPDATE cache_entry_statistics_state SET partitions_created=partitions_created+1 WHERE id=TRUE;
 IF NOT FOUND THEN RAISE EXCEPTION 'Cache statistics state missing'; END IF;
 UPDATE cache_partition_creation_intent SET consumed=TRUE WHERE generation=generation_id;
END $$;
REVOKE ALL ON FUNCTION create_cache_generation_partition(BIGINT) FROM PUBLIC;
DO $$ DECLARE signature TEXT; BEGIN
 FOREACH signature IN ARRAY ARRAY['guard_cache_partition_context()','admit_cache_partition_creation()','guard_cache_partition_intent()','create_cache_generation_partition(BIGINT)','validate_cache_generation_partition(BIGINT)'] LOOP
 EXECUTE format('ALTER FUNCTION %s SET search_path TO pg_catalog,%I,pg_temp',signature,current_schema());
 END LOOP;
END $$;
