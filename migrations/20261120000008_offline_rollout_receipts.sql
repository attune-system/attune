-- Offline deployment audit/control only. No accepted-work references or grants.
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM pg_namespace n,LATERAL aclexplode(COALESCE(n.nspacl,acldefault('n',n.nspowner))) a WHERE n.oid=current_schema()::regnamespace AND a.privilege_type='CREATE' AND a.grantee<>n.nspowner) THEN RAISE EXCEPTION 'Trusted owner-only installation schema required'; END IF;
END $$;
CREATE TABLE recorded_rollout_installation_identity (
 id BIGINT PRIMARY KEY CHECK(id=1), installation UUID NOT NULL UNIQUE DEFAULT gen_random_uuid()
);
INSERT INTO recorded_rollout_installation_identity(id) VALUES(1);
CREATE TABLE recorded_rollout_offline_receipt (
 id BIGSERIAL PRIMARY KEY,
 nonce UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
 body JSONB NOT NULL,
 issued TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
 expires TIMESTAMPTZ NOT NULL,
 actor NAME NOT NULL, backend INTEGER NOT NULL,
 CHECK(octet_length(body::TEXT)<=16384),CHECK(expires>issued AND expires<=issued+INTERVAL '15 minutes')
);
CREATE TABLE recorded_rollout_receipt_terminal (
 receipt BIGINT PRIMARY KEY REFERENCES recorded_rollout_offline_receipt(id),
 outcome TEXT NOT NULL CHECK(outcome IN ('consumed','revoked')),
 actor NAME NOT NULL,backend INTEGER NOT NULL,finished TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
REVOKE ALL ON recorded_rollout_installation_identity,recorded_rollout_offline_receipt,recorded_rollout_receipt_terminal FROM PUBLIC;
REVOKE ALL ON SEQUENCE recorded_rollout_offline_receipt_id_seq FROM PUBLIC;
CREATE FUNCTION guard_recorded_offline_metadata() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' OR NOT recorded_rollout_operator() THEN RAISE EXCEPTION 'Immutable operator-owned offline metadata' USING ERRCODE='42501'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER recorded_installation_immutable BEFORE INSERT OR UPDATE OR DELETE ON recorded_rollout_installation_identity FOR EACH ROW EXECUTE FUNCTION guard_recorded_offline_metadata();
CREATE TRIGGER recorded_offline_receipt_immutable BEFORE INSERT OR UPDATE OR DELETE ON recorded_rollout_offline_receipt FOR EACH ROW EXECUTE FUNCTION guard_recorded_offline_metadata();
CREATE TRIGGER recorded_terminal_immutable BEFORE INSERT OR UPDATE OR DELETE ON recorded_rollout_receipt_terminal FOR EACH ROW EXECUTE FUNCTION guard_recorded_offline_metadata();
CREATE FUNCTION recorded_rollout_installation() RETURNS JSONB LANGUAGE SQL STABLE AS $$
 SELECT jsonb_build_object('uuid',i.installation,'capability_role',recorded_rollout_operator_role(),'database',d.datname,'database_oid',d.oid::BIGINT,'schema',n.nspname,'schema_oid',n.oid::BIGINT)
 FROM recorded_rollout_installation_identity i,pg_database d,pg_namespace n,pg_proc p
 WHERE i.id=1 AND d.datname=current_database() AND p.oid=to_regprocedure('recorded_rollout_operator_role()') AND n.oid=p.pronamespace
$$;
CREATE FUNCTION issue_recorded_rollout_receipt(manifest JSONB) RETURNS JSONB LANGUAGE plpgsql SECURITY DEFINER AS $$
DECLARE prior recorded_rollout_active; target recorded_rollout_generation; rid BIGINT; token UUID; now_at TIMESTAMPTZ; until_at TIMESTAMPTZ; doc JSONB;
BEGIN
 IF NOT recorded_rollout_operator() THEN RAISE EXCEPTION 'Deployment operator role required' USING ERRCODE='42501'; END IF;
 IF recorded_rollout_installation() IS NULL THEN RAISE EXCEPTION 'Installation identity missing'; END IF;
 SELECT * INTO STRICT prior FROM recorded_rollout_active WHERE singleton FOR UPDATE;
 rid:=stage_recorded_rollout(manifest);
 SELECT * INTO STRICT target FROM recorded_rollout_generation WHERE id=rid;
 now_at:=clock_timestamp(); until_at:=now_at+INTERVAL '15 minutes'; token:=gen_random_uuid();
 doc:=jsonb_build_object('operation','activate','version',1,'installation',recorded_rollout_installation(),'nonce',token,
 'target',jsonb_build_object('generation',target.generation,'row_id',target.id,'digest',target.membership_digest,'snapshot',target.expected->'snapshot'),
 'prior',jsonb_build_object('row_id',prior.generation,'revision',prior.revision),'issued',now_at,'expires',until_at,'actor',session_user,'backend',pg_backend_pid());
 INSERT INTO recorded_rollout_offline_receipt(nonce,body,issued,expires,actor,backend) VALUES(token,doc,now_at,until_at,session_user,pg_backend_pid()) RETURNING id INTO rid;
 RETURN doc;
END $$;
CREATE FUNCTION consume_recorded_rollout_receipt(doc JSONB) RETURNS VOID LANGUAGE plpgsql SECURITY DEFINER AS $$
DECLARE prior recorded_rollout_active; held recorded_rollout_offline_receipt; target recorded_rollout_generation;
BEGIN
 IF NOT recorded_rollout_operator() THEN RAISE EXCEPTION 'Deployment operator role required' USING ERRCODE='42501'; END IF;
 IF recorded_rollout_installation() IS NULL THEN RAISE EXCEPTION 'Installation identity missing'; END IF;
 SELECT * INTO STRICT prior FROM recorded_rollout_active WHERE singleton FOR UPDATE;
 SELECT * INTO STRICT held FROM recorded_rollout_offline_receipt WHERE nonce=(doc->>'nonce')::UUID FOR UPDATE;
 IF held.body IS DISTINCT FROM doc OR doc->'installation' IS DISTINCT FROM recorded_rollout_installation()
 OR clock_timestamp()>=held.expires OR EXISTS(SELECT 1 FROM recorded_rollout_receipt_terminal WHERE receipt=held.id)
 OR (doc->'prior'->>'row_id')::BIGINT IS DISTINCT FROM prior.generation OR (doc->'prior'->>'revision')::BIGINT IS DISTINCT FROM prior.revision THEN RAISE EXCEPTION 'Offline receipt denied'; END IF;
 SELECT * INTO STRICT target FROM recorded_rollout_generation WHERE id=(doc->'target'->>'row_id')::BIGINT FOR SHARE;
 IF target.generation IS DISTINCT FROM (doc->'target'->>'generation')::UUID OR target.membership_digest IS DISTINCT FROM doc->'target'->>'digest' OR target.expected->'snapshot' IS DISTINCT FROM doc->'target'->'snapshot' THEN RAISE EXCEPTION 'Offline receipt target denied'; END IF;
 PERFORM activate_recorded_rollout(target.generation,target.membership_digest,prior.generation,prior.revision);
 INSERT INTO recorded_rollout_receipt_terminal(receipt,outcome,actor,backend) VALUES(held.id,'consumed',session_user,pg_backend_pid());
END $$;
CREATE FUNCTION revoke_recorded_rollout_receipt(doc JSONB) RETURNS VOID LANGUAGE plpgsql SECURITY DEFINER AS $$
DECLARE held recorded_rollout_offline_receipt;
BEGIN
 IF NOT recorded_rollout_operator() THEN RAISE EXCEPTION 'Deployment operator role required' USING ERRCODE='42501'; END IF;
 IF recorded_rollout_installation() IS NULL THEN RAISE EXCEPTION 'Installation identity missing'; END IF;
 SELECT * INTO STRICT held FROM recorded_rollout_offline_receipt WHERE nonce=(doc->>'nonce')::UUID FOR UPDATE;
 IF held.body IS DISTINCT FROM doc OR doc->'installation' IS DISTINCT FROM recorded_rollout_installation() THEN RAISE EXCEPTION 'Offline receipt denied'; END IF;
 INSERT INTO recorded_rollout_receipt_terminal(receipt,outcome,actor,backend) VALUES(held.id,'revoked',session_user,pg_backend_pid());
END $$;
-- Read the runner-owned ledger without granting the offline login ledger writes
-- or broad migration privileges. Missing ledger is an error, never invented proof.
CREATE FUNCTION recorded_rollout_admin_ledger() RETURNS TABLE(version BIGINT,checksum BYTEA,success BOOLEAN) LANGUAGE plpgsql SECURITY DEFINER AS $$
BEGIN
 IF NOT recorded_rollout_operator() THEN RAISE EXCEPTION 'Deployment operator role required' USING ERRCODE='42501'; END IF;
 IF recorded_rollout_installation() IS NULL THEN RAISE EXCEPTION 'Installation identity missing'; END IF;
 RETURN QUERY SELECT m.version,m.checksum,m.success FROM _sqlx_migrations m ORDER BY m.version;
END $$;
REVOKE ALL ON FUNCTION issue_recorded_rollout_receipt(JSONB),consume_recorded_rollout_receipt(JSONB),revoke_recorded_rollout_receipt(JSONB),recorded_rollout_admin_ledger() FROM PUBLIC;
DO $$ DECLARE signature TEXT; BEGIN
 FOREACH signature IN ARRAY ARRAY['guard_recorded_offline_metadata()','recorded_rollout_installation()','issue_recorded_rollout_receipt(JSONB)','consume_recorded_rollout_receipt(JSONB)','revoke_recorded_rollout_receipt(JSONB)','recorded_rollout_admin_ledger()'] LOOP
 EXECUTE format('ALTER FUNCTION %s SET search_path TO pg_catalog, %I, pg_temp',signature,current_schema());
 END LOOP;
END $$;
