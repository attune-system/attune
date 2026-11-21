-- Deployment metadata only: never an execution grant or receipt lifetime owner.
-- DBA provisions the installation-specific NOLOGIN capability named by
-- recorded_rollout_operator_role() and grants direct membership
-- ONLY to an offline deployment identity. No CREATEROLE or runtime grants here.
-- Require an owner-only CREATE ACL before capturing this trusted schema.
DO $$ BEGIN
    IF EXISTS(SELECT 1 FROM pg_namespace n,
        LATERAL aclexplode(COALESCE(n.nspacl,acldefault('n',n.nspowner))) a
        WHERE n.oid=current_schema()::regnamespace AND a.privilege_type='CREATE' AND a.grantee<>n.nspowner)
    THEN RAISE EXCEPTION 'Deployment membership requires owner-only installation schema CREATE'; END IF;
    EXECUTE format('CREATE FUNCTION recorded_rollout_operator_role() RETURNS NAME LANGUAGE SQL IMMUTABLE AS %L',
        'SELECT ' || quote_literal('attune_rollout_operator_' || replace(gen_random_uuid()::TEXT,'-','')) || '::NAME');
END $$;
CREATE FUNCTION recorded_rollout_operator() RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT EXISTS(SELECT 1 FROM pg_auth_members m
        JOIN pg_roles capability ON capability.oid=m.roleid
        JOIN pg_roles actor ON actor.oid=m.member
        WHERE capability.rolname=recorded_rollout_operator_role() AND actor.rolname=session_user)
$$;
CREATE FUNCTION recorded_membership_valid(body JSONB) RETURNS BOOLEAN
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE s JSONB; member JSONB; roles TEXT[] := '{}'; instances UUID[] := '{}'; boots UUID[] := '{}'; i UUID; b UUID; required INTEGER;
BEGIN
    IF octet_length(body::TEXT)>2097152 OR jsonb_typeof(body)<>'object' OR (SELECT count(*) FROM jsonb_object_keys(body))<>2 THEN RETURN FALSE; END IF;
    s:=body->'snapshot';
    IF jsonb_typeof(s) IS DISTINCT FROM 'object' OR (SELECT count(*) FROM jsonb_object_keys(s))<>5
       OR jsonb_typeof(s->'contract_version') IS DISTINCT FROM 'number'
       OR jsonb_typeof(s->'catalog_epoch') IS DISTINCT FROM 'number'
       OR jsonb_typeof(s->'catalog_revision') IS DISTINCT FROM 'number'
       OR jsonb_typeof(s->'release_digest') IS DISTINCT FROM 'string'
       OR jsonb_typeof(s->'rollout_generation') IS DISTINCT FROM 'string'
       OR (s->>'rollout_generation')::UUID::TEXT IS DISTINCT FROM s->>'rollout_generation'
       OR (s->>'contract_version')::INTEGER IS DISTINCT FROM 1
       OR NOT ((s->>'catalog_epoch')::INTEGER>0) OR NOT ((s->>'catalog_revision')::INTEGER>=0)
       OR NOT (s->>'release_digest' ~ '^[0-9a-f]{64}$')
       OR (s->>'rollout_generation')::UUID='00000000-0000-0000-0000-000000000000'::UUID
       OR s->>'catalog_epoch' IS NULL OR s->>'catalog_revision' IS NULL
       OR s->>'release_digest' IS NULL OR s->>'rollout_generation' IS NULL THEN RETURN FALSE; END IF;
    IF jsonb_typeof(body->'members') IS DISTINCT FROM 'array' OR jsonb_array_length(body->'members') NOT BETWEEN 1 AND 4096 THEN RETURN FALSE; END IF;
    FOR member IN SELECT value FROM jsonb_array_elements(body->'members') LOOP
        IF jsonb_typeof(member)<>'object' OR (SELECT count(*) FROM jsonb_object_keys(member))<>4 THEN RETURN FALSE; END IF;
        IF jsonb_typeof(member->'role') IS DISTINCT FROM 'string' OR jsonb_typeof(member->'instance') IS DISTINCT FROM 'string' OR jsonb_typeof(member->'boot') IS DISTINCT FROM 'string' OR jsonb_typeof(member->'required_mask') IS DISTINCT FROM 'number' THEN RETURN FALSE; END IF;
        required:=CASE member->>'role' WHEN 'api' THEN 15 WHEN 'notifier' THEN 16 WHEN 'executor' THEN 65 WHEN 'worker' THEN 40 END;
        i:=(member->>'instance')::UUID; b:=(member->>'boot')::UUID;
        IF i::TEXT IS DISTINCT FROM member->>'instance' OR b::TEXT IS DISTINCT FROM member->>'boot' OR required IS NULL OR (member->>'required_mask')::INTEGER IS DISTINCT FROM required OR i IS NULL OR b IS NULL
           OR i='00000000-0000-0000-0000-000000000000'::UUID OR b='00000000-0000-0000-0000-000000000000'::UUID
           OR i=ANY(instances) OR b=ANY(boots) THEN RETURN FALSE; END IF;
        roles:=array_append(roles,member->>'role'); instances:=array_append(instances,i); boots:=array_append(boots,b);
    END LOOP;
    RETURN roles @> ARRAY['api','notifier','executor','worker'];
EXCEPTION WHEN OTHERS THEN RETURN FALSE;
END $$;
CREATE FUNCTION recorded_membership_digest(body JSONB) RETURNS TEXT LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT encode(sha256(convert_to(body::TEXT,'UTF8')),'hex')
$$;
CREATE FUNCTION canonical_recorded_membership(body JSONB) RETURNS JSONB LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT jsonb_set(body,'{members}',jsonb_agg(value ORDER BY value->>'instance')) FROM jsonb_array_elements(body->'members')
$$;
CREATE TABLE recorded_rollout_generation (
    id BIGSERIAL PRIMARY KEY,
    generation UUID NOT NULL UNIQUE,
    expected JSONB NOT NULL CHECK(recorded_membership_valid(expected)) CHECK(expected=canonical_recorded_membership(expected)),
    membership_digest TEXT GENERATED ALWAYS AS (recorded_membership_digest(expected)) STORED,
    CHECK(generation=(expected->'snapshot'->>'rollout_generation')::UUID)
);
-- All generation rows are staged and immutable. Active state is this single
-- pointer, not a mutation of the expected set or a vote by observed services.
CREATE TABLE recorded_rollout_active (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK(singleton),
    generation BIGINT REFERENCES recorded_rollout_generation(id),
    revision BIGINT NOT NULL DEFAULT 0 CHECK(revision>=0),
    actor NAME, backend INTEGER, activated TIMESTAMPTZ
);
INSERT INTO recorded_rollout_active(singleton,generation) VALUES(TRUE,NULL);
REVOKE ALL ON recorded_rollout_generation,recorded_rollout_active FROM PUBLIC;
REVOKE ALL ON SEQUENCE recorded_rollout_generation_id_seq FROM PUBLIC;
CREATE FUNCTION guard_recorded_rollout_write() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT recorded_rollout_operator() THEN RAISE EXCEPTION 'Deployment operator role required' USING ERRCODE='42501'; END IF;
    IF TG_TABLE_NAME='recorded_rollout_generation' AND TG_OP<>'INSERT' THEN
        RAISE EXCEPTION 'Staged deployment membership is immutable';
    END IF;
    IF TG_TABLE_NAME='recorded_rollout_active' THEN
        IF TG_OP<>'UPDATE' THEN RAISE EXCEPTION 'Only exact active generation selection is supported'; END IF;
        IF NEW.singleton IS DISTINCT FROM OLD.singleton OR NEW.generation IS NULL OR NEW.revision<>OLD.revision+1 OR NEW.actor IS DISTINCT FROM session_user OR NEW.backend IS DISTINCT FROM pg_backend_pid() OR NEW.activated IS NULL THEN
            RAISE EXCEPTION 'Only exact active generation selection is supported';
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER recorded_generation_guard BEFORE INSERT OR UPDATE OR DELETE ON recorded_rollout_generation FOR EACH ROW EXECUTE FUNCTION guard_recorded_rollout_write();
CREATE TRIGGER recorded_active_guard BEFORE INSERT OR UPDATE OR DELETE ON recorded_rollout_active FOR EACH ROW EXECUTE FUNCTION guard_recorded_rollout_write();

-- Mutation functions alone receive table-owner rights. Callers still require
-- exact direct operator membership; SECURITY DEFINER never supplies that fact.
CREATE FUNCTION stage_recorded_rollout(body JSONB) RETURNS BIGINT
LANGUAGE plpgsql SECURITY DEFINER AS $$
DECLARE held recorded_rollout_generation; uid UUID;
BEGIN
    IF NOT recorded_rollout_operator() THEN RAISE EXCEPTION 'Deployment operator role required' USING ERRCODE='42501'; END IF;
    IF NOT recorded_membership_valid(body) THEN RAISE EXCEPTION 'Invalid deployment membership'; END IF;
    body:=canonical_recorded_membership(body);
    uid:=(body->'snapshot'->>'rollout_generation')::UUID;
    INSERT INTO recorded_rollout_generation(generation,expected) VALUES(uid,body) ON CONFLICT(generation) DO NOTHING;
    SELECT * INTO STRICT held FROM recorded_rollout_generation WHERE generation=uid;
    IF held.expected IS DISTINCT FROM body THEN RAISE EXCEPTION 'Conflicting deployment generation'; END IF;
    RETURN held.id;
END $$;
CREATE FUNCTION activate_recorded_rollout(target UUID, digest TEXT, prior BIGINT, prior_revision BIGINT) RETURNS VOID
LANGUAGE plpgsql SECURITY DEFINER AS $$
DECLARE active recorded_rollout_active; staged recorded_rollout_generation;
BEGIN
    IF NOT recorded_rollout_operator() THEN RAISE EXCEPTION 'Deployment operator role required' USING ERRCODE='42501'; END IF;
    SELECT * INTO STRICT active FROM recorded_rollout_active WHERE singleton FOR UPDATE;
    IF active.generation IS DISTINCT FROM prior OR active.revision IS DISTINCT FROM prior_revision THEN RAISE EXCEPTION 'Stale deployment activation'; END IF;
    SELECT * INTO STRICT staged FROM recorded_rollout_generation WHERE generation=target FOR SHARE;
    IF NOT recorded_membership_valid(staged.expected) OR staged.membership_digest IS DISTINCT FROM digest
       OR staged.membership_digest IS DISTINCT FROM recorded_membership_digest(staged.expected) THEN RAISE EXCEPTION 'Invalid deployment activation target'; END IF;
    UPDATE recorded_rollout_active SET generation=staged.id,revision=revision+1,actor=session_user,backend=pg_backend_pid(),activated=clock_timestamp() WHERE singleton;
END $$;
REVOKE ALL ON FUNCTION stage_recorded_rollout(JSONB),activate_recorded_rollout(UUID,TEXT,BIGINT,BIGINT) FROM PUBLIC;
-- Resolve the installation schema at migration time, never a service-controlled
-- or caller-controlled search_path. The installation schema must be DBA-owned.
DO $$ BEGIN
    EXECUTE format('ALTER FUNCTION stage_recorded_rollout(JSONB) SET search_path TO pg_catalog, %I, pg_temp',current_schema());
    EXECUTE format('ALTER FUNCTION activate_recorded_rollout(UUID,TEXT,BIGINT,BIGINT) SET search_path TO pg_catalog, %I, pg_temp',current_schema());
END $$;
DO $$ DECLARE signature TEXT; BEGIN
    FOREACH signature IN ARRAY ARRAY['recorded_rollout_operator_role()','recorded_rollout_operator()','recorded_membership_valid(JSONB)','recorded_membership_digest(JSONB)','guard_recorded_rollout_write()','canonical_recorded_membership(JSONB)'] LOOP
        EXECUTE format('ALTER FUNCTION %s SET search_path TO pg_catalog, %I, pg_temp',signature,current_schema());
    END LOOP;
END $$;
