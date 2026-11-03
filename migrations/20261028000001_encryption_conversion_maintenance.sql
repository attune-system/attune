-- Forward-only, operator-gated maintenance. No existing migration bytes change.
-- A database administrator must provision attune_encryption_maintenance NOLOGIN
-- and grant membership ONLY to the offline operator. Installation itself does
-- not require CREATEROLE and never grants this capability to a runtime identity.
CREATE TABLE encryption_conversion_run (
    id BIGSERIAL PRIMARY KEY,
    run_uid UUID NOT NULL UNIQUE,
    actor NAME NOT NULL,
    backend INTEGER NOT NULL,
    transaction_id TEXT NOT NULL,
    report JSONB,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    finished TIMESTAMPTZ
);
CREATE TABLE encryption_conversion_change (
    id BIGSERIAL PRIMARY KEY,
    run BIGINT NOT NULL REFERENCES encryption_conversion_run(id),
    relation OID NOT NULL,
    identity JSONB NOT NULL,
    before_digest TEXT NOT NULL,
    after_digest TEXT NOT NULL,
    UNIQUE(run,relation,identity)
);
REVOKE ALL ON encryption_conversion_run, encryption_conversion_change FROM PUBLIC;
REVOKE ALL ON SEQUENCE encryption_conversion_run_id_seq, encryption_conversion_change_id_seq FROM PUBLIC;

CREATE FUNCTION encryption_conversion_operator() RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT EXISTS(SELECT 1 FROM pg_auth_members m
        JOIN pg_roles capability ON capability.oid=m.roleid
        JOIN pg_roles actor ON actor.oid=m.member
        WHERE capability.rolname='attune_encryption_maintenance' AND actor.rolname=session_user)
$$;
CREATE FUNCTION encryption_conversion_row_digest(value JSONB) RETURNS TEXT LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT encode(sha256(convert_to(value::TEXT,'UTF8')),'hex')
$$;

CREATE FUNCTION encryption_conversion_transition(target OID, previous JSONB, replacement JSONB)
RETURNS BOOLEAN LANGUAGE plpgsql SECURITY DEFINER AS $$
BEGIN
    IF NOT encryption_conversion_operator() THEN RETURN FALSE; END IF;
    RETURN EXISTS(SELECT 1 FROM encryption_conversion_change c JOIN encryption_conversion_run r ON r.id=c.run
        WHERE NOT r.completed AND r.actor=session_user AND r.backend=pg_backend_pid()
          AND r.transaction_id=pg_current_xact_id()::TEXT AND c.relation=target
          AND previous @> c.identity AND replacement @> c.identity
          AND c.before_digest=encryption_conversion_row_digest(previous)
          AND c.after_digest=encryption_conversion_row_digest(replacement));
END $$;

CREATE FUNCTION encryption_conversion_begin(uid UUID) RETURNS BIGINT
LANGUAGE plpgsql SECURITY DEFINER AS $$
DECLARE owned BIGINT;
BEGIN
    IF NOT encryption_conversion_operator() THEN RAISE EXCEPTION 'Offline encryption maintenance role required'; END IF;
    IF NOT file_evidence_graph_held(TRUE) THEN RAISE EXCEPTION 'Encryption conversion requires exclusive graph admission'; END IF;
    IF NOT pg_try_advisory_xact_lock(hashtextextended('attune:encryption-conversion',0)) THEN
        RAISE EXCEPTION 'Another encryption conversion owns maintenance admission';
    END IF;
    INSERT INTO encryption_conversion_run(run_uid,actor,backend,transaction_id)
        VALUES(uid,session_user,pg_backend_pid(),pg_current_xact_id()::TEXT) RETURNING id INTO owned;
    RETURN owned;
END $$;

CREATE FUNCTION encryption_conversion_lock() RETURNS VOID LANGUAGE plpgsql SECURITY DEFINER AS $$
BEGIN
    IF NOT encryption_conversion_operator() THEN RAISE EXCEPTION 'Offline encryption maintenance role required'; END IF;
    IF NOT pg_try_advisory_xact_lock(hashtextextended('attune:encryption-conversion',0)) THEN
        RAISE EXCEPTION 'Another encryption conversion owns maintenance admission';
    END IF;
    PERFORM file_evidence_gc_entry();
    LOCK TABLE key,execution_secret_value,runtime_input_receipt,runtime_accepted_input,
        runtime_native_input_snapshot,event_origin_snapshot,runtime_native_success_metadata_observation,
        runtime_native_iteration_outcome_member,file_evidence_owner,file_evidence_pin,artifact_file_origin
        IN ACCESS EXCLUSIVE MODE;
END $$;

CREATE FUNCTION encryption_conversion_apply(owned BIGINT, target TEXT, identity JSONB, expected TEXT, changes JSONB)
RETURNS VOID LANGUAGE plpgsql SECURITY DEFINER AS $$
DECLARE allowed TEXT[]; predicate TEXT; schema_name NAME; relation OID;
        previous JSONB; replacement JSONB; actual JSONB; assignments TEXT; affected BIGINT;
BEGIN
    IF NOT encryption_conversion_operator() OR NOT EXISTS(SELECT 1 FROM encryption_conversion_run r
        WHERE r.id=owned AND NOT r.completed AND r.actor=session_user AND r.backend=pg_backend_pid()
          AND r.transaction_id=pg_current_xact_id()::TEXT) THEN
        RAISE EXCEPTION 'No transaction-owned encryption conversion';
    END IF;
    CASE target
      WHEN 'key' THEN allowed:=ARRAY['value','encryption_key_hash'];
      WHEN 'execution_secret_value' THEN allowed:=ARRAY['encryption_key_hash'];
      WHEN 'runtime_input_receipt' THEN allowed:=ARRAY['encryption_key_hash'];
      WHEN 'runtime_accepted_input' THEN allowed:=ARRAY['body','body_digest','encryption_key_hash'];
      WHEN 'runtime_native_input_snapshot' THEN allowed:=ARRAY['body','body_digest','encryption_key_hash'];
      WHEN 'event_origin_snapshot' THEN allowed:=ARRAY['snapshot','snapshot_digest'];
      WHEN 'runtime_native_success_metadata_observation' THEN allowed:=ARRAY['observation'];
      WHEN 'runtime_native_iteration_outcome_member' THEN allowed:=ARRAY['member'];
      ELSE RAISE EXCEPTION 'Unsupported encryption maintenance relation';
    END CASE;
    IF jsonb_typeof(changes)<>'object' OR changes='{}'::JSONB
        OR EXISTS(SELECT 1 FROM jsonb_object_keys(changes) AS keys(k) WHERE NOT k=ANY(allowed)) THEN
        RAISE EXCEPTION 'Encryption maintenance cannot modify identity, ownership, provenance or unrelated columns';
    END IF;
    IF target='runtime_native_iteration_outcome_member' THEN
        IF identity - ARRAY['snapshot','ordinal'] <> '{}'::JSONB OR (SELECT count(*) FROM jsonb_object_keys(identity))<>2 THEN
            RAISE EXCEPTION 'Invalid encryption maintenance row identity';
        END IF;
        predicate:='t.snapshot=($1->>''snapshot'')::BIGINT AND t.ordinal=($1->>''ordinal'')::INTEGER';
    ELSIF target='runtime_native_success_metadata_observation' THEN
        IF identity - 'snapshot' <> '{}'::JSONB OR (SELECT count(*) FROM jsonb_object_keys(identity))<>1 THEN RAISE EXCEPTION 'Invalid encryption maintenance row identity'; END IF;
        predicate:='t.snapshot=($1->>''snapshot'')::BIGINT';
    ELSE
        IF identity - 'id' <> '{}'::JSONB OR (SELECT count(*) FROM jsonb_object_keys(identity))<>1 THEN RAISE EXCEPTION 'Invalid encryption maintenance row identity'; END IF;
        predicate:='t.id=($1->>''id'')::BIGINT';
    END IF;
    relation:=to_regclass(target);
    SELECT n.nspname INTO STRICT schema_name FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
        WHERE c.oid=relation AND c.relnamespace=(SELECT relnamespace FROM pg_class WHERE oid='encryption_conversion_run'::regclass);
    EXECUTE format('SELECT to_jsonb(t) FROM %I.%I t WHERE %s FOR UPDATE',schema_name,target,predicate) INTO STRICT previous USING identity;
    IF previous IS NULL OR encryption_conversion_row_digest(previous) IS DISTINCT FROM expected THEN
        RAISE EXCEPTION 'Encryption conversion source changed or plan does not match';
    END IF;
    replacement:=previous || changes;
    IF target='key' THEN replacement:=replacement || jsonb_build_object('updated',now()); END IF;
    INSERT INTO encryption_conversion_change(run,relation,identity,before_digest,after_digest)
        VALUES(owned,relation,identity,expected,encryption_conversion_row_digest(replacement));
    SELECT string_agg(format('%I=n.%I',key,key),',') INTO assignments FROM jsonb_object_keys(changes) AS keys(key);
    EXECUTE format('UPDATE %I.%I t SET %s FROM jsonb_populate_record(NULL::%I.%I,$2) n WHERE %s',schema_name,target,assignments,schema_name,target,predicate)
        USING identity,replacement;
    GET DIAGNOSTICS affected=ROW_COUNT;
    IF affected<>1 THEN RAISE EXCEPTION 'Encryption conversion did not update exactly one planned row'; END IF;
    EXECUTE format('SELECT to_jsonb(t) FROM %I.%I t WHERE %s',schema_name,target,predicate) INTO STRICT actual USING identity;
    IF encryption_conversion_row_digest(actual) IS DISTINCT FROM encryption_conversion_row_digest(replacement) THEN
        RAISE EXCEPTION 'Encryption conversion produced an unplanned row transition';
    END IF;
END $$;

CREATE FUNCTION encryption_conversion_finish(owned BIGINT, counts JSONB)
RETURNS VOID LANGUAGE plpgsql SECURITY DEFINER AS $$
BEGIN
    IF NOT encryption_conversion_operator() THEN RAISE EXCEPTION 'Offline encryption maintenance role required'; END IF;
    IF jsonb_typeof(counts)<>'object' OR EXISTS(SELECT 1 FROM jsonb_each(counts) e
        WHERE jsonb_typeof(e.value) NOT IN ('number','boolean') OR e.key NOT IN
        ('keys_converted','keys_already_current','field_verifiers_converted','fields_already_current','graphs_converted','applied')) THEN
        RAISE EXCEPTION 'Encryption maintenance audit accepts counts only';
    END IF;
    UPDATE encryption_conversion_run SET completed=TRUE,report=counts,finished=clock_timestamp()
      WHERE id=owned AND actor=session_user AND backend=pg_backend_pid() AND transaction_id=pg_current_xact_id()::TEXT AND NOT completed;
    IF NOT FOUND THEN RAISE EXCEPTION 'No transaction-owned encryption conversion'; END IF;
END $$;
CREATE FUNCTION encryption_conversion_complete_guard() RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM encryption_conversion_run WHERE id=NEW.id AND NOT completed) THEN
        RAISE EXCEPTION 'Incomplete encryption conversion cannot be committed';
    END IF;
    RETURN NEW;
END $$;
CREATE CONSTRAINT TRIGGER encryption_conversion_complete_guard AFTER INSERT OR UPDATE ON encryption_conversion_run
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION encryption_conversion_complete_guard();

-- Preserve every original guard body verbatim. The additional UPDATE-only arm
-- requires a privileged exact-row transition issued by the function above.
-- INSERT, DELETE and all ordinary runtime UPDATE behavior remain unchanged.
DO $$ DECLARE name TEXT; definition TEXT; namespace NAME:=current_schema(); BEGIN
    FOREACH name IN ARRAY ARRAY['guard_runtime_schema_binding','guard_event_origin_snapshot',
        'guard_native_snapshot_archive','guard_event_secret_values','guard_sensor_generation_secret_values',
        'guard_sensor_rule_delivery_secret_values','guard_component_configuration_secret_values',
        'file_evidence_guard_accepted_archive','file_evidence_guard_native_archive',
        'protect_native_success_metadata','protect_native_iteration_outcome'] LOOP
        SELECT p.prosrc INTO STRICT definition FROM pg_proc p JOIN pg_namespace n ON p.pronamespace=n.oid
          WHERE n.nspname=namespace AND p.proname=name AND p.pronargs=0 AND p.prorettype='trigger'::regtype;
        IF definition ~ 'encryption_conversion_transition' OR definition !~ '\mBEGIN\M' THEN
            RAISE EXCEPTION 'Unexpected encryption maintenance guard definition';
        END IF;
        definition:=regexp_replace(definition,'\mBEGIN\M',
          'BEGIN IF TG_OP=''UPDATE'' AND encryption_conversion_transition(TG_RELID,to_jsonb(OLD),to_jsonb(NEW)) THEN RETURN NEW; END IF;');
        EXECUTE format('CREATE OR REPLACE FUNCTION %I.%I() RETURNS TRIGGER LANGUAGE plpgsql AS %L',namespace,name,definition);
        EXECUTE format('ALTER FUNCTION %I.%I() SET search_path TO %I,pg_temp',namespace,name,namespace);
    END LOOP;
    -- Pin privileged lookup paths; temporary tables cannot impersonate permits.
    FOREACH name IN ARRAY ARRAY['encryption_conversion_operator','encryption_conversion_row_digest',
      'encryption_conversion_transition','encryption_conversion_begin',
      'encryption_conversion_lock',
      'encryption_conversion_apply','encryption_conversion_finish',
      'encryption_conversion_complete_guard'] LOOP
        FOR definition IN SELECT p.oid::regprocedure::TEXT FROM pg_proc p JOIN pg_namespace n ON p.pronamespace=n.oid
            WHERE n.nspname=namespace AND p.proname=name LOOP
            EXECUTE format('ALTER FUNCTION %s SET search_path TO %I,pg_temp',definition,namespace);
        END LOOP;
    END LOOP;
END $$;
