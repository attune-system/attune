-- Preserve exact-row conversion planning when the database advances the Key's
-- internal revision. Caller-controlled columns and every existing fence stay closed.
CREATE OR REPLACE FUNCTION encryption_conversion_apply(owned BIGINT, target TEXT, identity JSONB, expected TEXT, changes JSONB)
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
    IF target='key' THEN
        -- Derived only from the verified previous row, exactly as the Key trigger.
        replacement:=replacement || jsonb_build_object('updated',now(),
            'input_revision',(previous->>'input_revision')::BIGINT + 1);
    END IF;
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

-- CREATE OR REPLACE must retain the privileged function's pinned lookup path.
DO $$ DECLARE namespace NAME:=current_schema(); BEGIN
    EXECUTE format('ALTER FUNCTION %I.encryption_conversion_apply(bigint,text,jsonb,text,jsonb) SET search_path TO %I,pg_temp',namespace,namespace);
END $$;
