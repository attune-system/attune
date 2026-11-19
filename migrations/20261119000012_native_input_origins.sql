-- Only actually issued native input obligations are captured. Existing authority,
-- approval and receipt identities keep their own immutable/revocation rules.
-- Extend the existing union so preceding sensor/candidate kinds remain admitted.
DO $$
DECLARE existing TEXT;
BEGIN
    SELECT pg_get_expr(conbin, conrelid) INTO existing FROM pg_constraint
      WHERE conrelid='workload_source_epoch'::regclass
        AND conname='workload_source_epoch_kind_check';
    IF existing IS NULL THEN RAISE EXCEPTION 'Workload source kind constraint is unavailable'; END IF;
    ALTER TABLE workload_source_epoch DROP CONSTRAINT workload_source_epoch_kind_check;
    EXECUTE format('ALTER TABLE workload_source_epoch ADD CONSTRAINT workload_source_epoch_kind_check CHECK ((%s) OR kind = ANY (%L::text[]))',
      existing, ARRAY['input_sponsor','input_contributors','input_secret_buffer','input_provenance_buffer',
        'input_execution_presence','input_workflow_presence',
        'input_key_owner','input_key_class','input_identity_ref','input_sensor_live',
        'input_pack_owner','input_pack_primary','input_action_owner','input_sensor_owner',
        'input_rule_owner','input_queue_owner','input_trigger_owner','input_execution_owner',
        'input_queue_item_owner','input_inquiry_owner','input_artifact_owner',
        'input_artifact_visibility','input_cache_owner','input_artifact_version']);
END $$;

CREATE FUNCTION native_input_source_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE affected BIGINT; source_kind TEXT;
BEGIN
    IF TG_TABLE_NAME='identity' THEN
        IF TG_OP='DELETE' OR OLD.frozen IS DISTINCT FROM NEW.frozen THEN
            FOR affected IN SELECT pack FROM pack_runtime_identity WHERE identity=OLD.id ORDER BY pack
            LOOP PERFORM bump_workload_source_epoch('input_sponsor',affected); END LOOP;
        END IF;
        IF TG_OP='DELETE' OR OLD.login IS DISTINCT FROM NEW.login THEN
            PERFORM bump_workload_source_epoch('input_identity_ref',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME='pack_runtime_identity' THEN
        IF TG_OP<>'INSERT' THEN PERFORM bump_workload_source_epoch('input_sponsor',OLD.pack); END IF;
        IF TG_OP<>'DELETE' AND (TG_OP='INSERT' OR NEW.pack IS DISTINCT FROM OLD.pack) THEN
            PERFORM bump_workload_source_epoch('input_sponsor',NEW.pack);
        END IF;
    ELSIF TG_TABLE_NAME='pack' THEN
        IF TG_OP='DELETE' OR OLD.ref IS DISTINCT FROM NEW.ref THEN
            PERFORM bump_workload_source_epoch('input_pack_owner',OLD.id);
            PERFORM bump_workload_source_epoch('input_sponsor',OLD.id);
        END IF;
        IF TG_OP='DELETE' OR ROW(OLD.installed_by,OLD.is_standard) IS DISTINCT FROM ROW(NEW.installed_by,NEW.is_standard) THEN
            PERFORM bump_workload_source_epoch('input_pack_primary',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME='key' THEN
        IF TG_OP='DELETE' OR ROW(OLD.id,OLD.ref,OLD.owner_type,OLD.owner,OLD.owner_identity,
             OLD.owner_pack,OLD.owner_action,OLD.owner_sensor,OLD.owner_pack_ref,OLD.owner_action_ref,OLD.owner_sensor_ref)
          IS DISTINCT FROM ROW(NEW.id,NEW.ref,NEW.owner_type,NEW.owner,NEW.owner_identity,
             NEW.owner_pack,NEW.owner_action,NEW.owner_sensor,NEW.owner_pack_ref,NEW.owner_action_ref,NEW.owner_sensor_ref) THEN
            PERFORM bump_workload_source_epoch('input_key_owner',OLD.id);
        END IF;
        IF TG_OP='DELETE' OR OLD.encrypted IS DISTINCT FROM NEW.encrypted THEN
            PERFORM bump_workload_source_epoch('input_key_class',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME='execution_secret_value' THEN
        IF TG_OP='DELETE' OR ROW(OLD.id,OLD.entity_type,OLD.entity_id,OLD.json_path,OLD.source_kind,
             OLD.source_ref,OLD.encrypted_value,OLD.encryption_key_hash)
          IS DISTINCT FROM ROW(NEW.id,NEW.entity_type,NEW.entity_id,NEW.json_path,NEW.source_kind,
             NEW.source_ref,NEW.encrypted_value,NEW.encryption_key_hash) THEN
            PERFORM bump_workload_source_epoch('input_secret_buffer',OLD.id);
        END IF;
        IF TG_OP='DELETE' OR ROW(OLD.id,OLD.entity_type,OLD.entity_id,OLD.json_path,OLD.source_kind,OLD.source_ref)
          IS DISTINCT FROM ROW(NEW.id,NEW.entity_type,NEW.entity_id,NEW.json_path,NEW.source_kind,NEW.source_ref) THEN
            PERFORM bump_workload_source_epoch('input_provenance_buffer',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME IN ('action','sensor','rule','work_queue','trigger') THEN
        source_kind=CASE TG_TABLE_NAME WHEN 'action' THEN 'input_action_owner' WHEN 'sensor' THEN 'input_sensor_owner'
            WHEN 'rule' THEN 'input_rule_owner' WHEN 'work_queue' THEN 'input_queue_owner' ELSE 'input_trigger_owner' END;
        IF TG_OP='DELETE' OR ROW(OLD.id,OLD.ref,OLD.pack,OLD.management_origin)
          IS DISTINCT FROM ROW(NEW.id,NEW.ref,NEW.pack,NEW.management_origin) THEN
            PERFORM bump_workload_source_epoch(source_kind,OLD.id);
        END IF;
        IF TG_TABLE_NAME='sensor' THEN
            IF TG_OP='DELETE' OR OLD.pack_ref IS DISTINCT FROM NEW.pack_ref THEN
                PERFORM bump_workload_source_epoch('input_sensor_owner',OLD.id);
            END IF;
            IF TG_OP='DELETE' OR OLD.retired_at IS DISTINCT FROM NEW.retired_at THEN
                PERFORM bump_workload_source_epoch('input_sensor_live',OLD.id);
            END IF;
        END IF;
    ELSIF TG_TABLE_NAME='workflow_execution' THEN
        IF TG_OP='DELETE' OR OLD.execution IS DISTINCT FROM NEW.execution THEN
            PERFORM bump_workload_source_epoch('input_workflow_presence',OLD.execution);
        END IF;
    ELSIF TG_TABLE_NAME='execution' THEN
        IF TG_OP='DELETE' THEN PERFORM bump_workload_source_epoch('input_execution_presence',OLD.id); END IF;
        IF TG_OP='DELETE' OR ROW(OLD.id,OLD.action_ref,OLD.pack_release,OLD.pack_release_digest,OLD.executable_snapshot)
          IS DISTINCT FROM ROW(NEW.id,NEW.action_ref,NEW.pack_release,NEW.pack_release_digest,NEW.executable_snapshot) THEN
            PERFORM bump_workload_source_epoch('input_execution_owner',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME='work_queue_item' THEN
        IF TG_OP='DELETE' OR ROW(OLD.id,OLD.queue,OLD.queue_ref) IS DISTINCT FROM ROW(NEW.id,NEW.queue,NEW.queue_ref) THEN
            PERFORM bump_workload_source_epoch('input_queue_item_owner',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME='inquiry' THEN
        IF TG_OP='DELETE' OR ROW(OLD.id,OLD.created_by_execution) IS DISTINCT FROM ROW(NEW.id,NEW.created_by_execution) THEN
            PERFORM bump_workload_source_epoch('input_inquiry_owner',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME='artifact' THEN
        IF TG_OP='DELETE' OR ROW(OLD.id,OLD.ref,OLD.scope,OLD.owner) IS DISTINCT FROM ROW(NEW.id,NEW.ref,NEW.scope,NEW.owner) THEN
            PERFORM bump_workload_source_epoch('input_artifact_owner',OLD.id);
        END IF;
        IF TG_OP='DELETE' OR OLD.visibility IS DISTINCT FROM NEW.visibility THEN
            PERFORM bump_workload_source_epoch('input_artifact_visibility',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME='artifact_version' THEN
        IF TG_OP='DELETE' OR ROW(OLD.id,OLD.artifact,OLD.execution,OLD.body_state,
             OLD.meta->'log_encoding',OLD.meta->'encrypted_log_manifest')
          IS DISTINCT FROM ROW(NEW.id,NEW.artifact,NEW.execution,NEW.body_state,
             NEW.meta->'log_encoding',NEW.meta->'encrypted_log_manifest') THEN
            PERFORM bump_workload_source_epoch('input_artifact_version',OLD.id);
        END IF;
    ELSIF TG_TABLE_NAME='cache_namespace' THEN
        IF TG_OP='DELETE' OR ROW(OLD.id,OLD.namespace,OLD.definition_ref,OLD.owner_type,OLD.owner_identity,
             OLD.owner_pack,OLD.owner_action,OLD.owner_sensor,OLD.owner_pack_ref,OLD.owner_action_ref,
             OLD.owner_sensor_ref,OLD.managing_pack,OLD.management_origin)
          IS DISTINCT FROM ROW(NEW.id,NEW.namespace,NEW.definition_ref,NEW.owner_type,NEW.owner_identity,
             NEW.owner_pack,NEW.owner_action,NEW.owner_sensor,NEW.owner_pack_ref,NEW.owner_action_ref,
             NEW.owner_sensor_ref,NEW.managing_pack,NEW.management_origin) THEN
            PERFORM bump_workload_source_epoch('input_cache_owner',OLD.id);
        END IF;
    END IF;
    RETURN NULL;
END $$;

-- Watch the captured contributor group, never the current installer, requester,
-- permission set, role, Key value, output data or human activity.
CREATE FUNCTION native_input_contributor_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE previous_execution BIGINT; next_execution BIGINT; affected BIGINT;
BEGIN
    IF TG_OP='UPDATE' AND to_jsonb(OLD) IS NOT DISTINCT FROM to_jsonb(NEW) THEN RETURN NULL; END IF;
    IF TG_TABLE_NAME='recorded_queue_execution_input' THEN
        IF TG_OP<>'INSERT' THEN previous_execution=OLD.execution; END IF;
        IF TG_OP<>'DELETE' THEN next_execution=NEW.execution; END IF;
    ELSE
        IF TG_OP<>'INSERT' AND OLD.owner_kind='execution' THEN previous_execution=OLD.owner_id; END IF;
        IF TG_OP<>'DELETE' AND NEW.owner_kind='execution' THEN next_execution=NEW.owner_id; END IF;
    END IF;
    FOR affected IN SELECT DISTINCT id FROM unnest(ARRAY[previous_execution,next_execution]) AS source(id)
        WHERE id IS NOT NULL ORDER BY id
    LOOP PERFORM bump_workload_source_epoch('input_contributors',affected); END LOOP;
    RETURN NULL;
END $$;

CREATE TRIGGER native_input_identity_epoch AFTER UPDATE OR DELETE ON identity FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_sponsor_epoch AFTER INSERT OR UPDATE OR DELETE ON pack_runtime_identity FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_pack_epoch AFTER UPDATE OR DELETE ON pack FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_key_epoch AFTER UPDATE OR DELETE ON key FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_buffer_epoch AFTER UPDATE OR DELETE ON execution_secret_value FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_action_epoch AFTER UPDATE OR DELETE ON action FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_sensor_epoch AFTER UPDATE OR DELETE ON sensor FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_rule_epoch AFTER UPDATE OR DELETE ON rule FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_queue_epoch AFTER UPDATE OR DELETE ON work_queue FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_trigger_epoch AFTER UPDATE OR DELETE ON trigger FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_execution_epoch AFTER UPDATE OR DELETE ON execution FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_workflow_presence_epoch AFTER UPDATE OR DELETE ON workflow_execution FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_item_epoch AFTER UPDATE OR DELETE ON work_queue_item FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_inquiry_epoch AFTER UPDATE OR DELETE ON inquiry FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_artifact_epoch AFTER UPDATE OR DELETE ON artifact FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_version_epoch AFTER UPDATE OR DELETE ON artifact_version FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_cache_epoch AFTER UPDATE OR DELETE ON cache_namespace FOR EACH ROW EXECUTE FUNCTION native_input_source_epoch();
CREATE TRIGGER native_input_recorded_contributors_epoch AFTER INSERT OR UPDATE OR DELETE ON recorded_execution_input_authority FOR EACH ROW EXECUTE FUNCTION native_input_contributor_epoch();
CREATE TRIGGER native_input_queue_contributors_epoch AFTER INSERT OR UPDATE OR DELETE ON recorded_queue_execution_input FOR EACH ROW EXECUTE FUNCTION native_input_contributor_epoch();
CREATE TRIGGER native_input_rule_contributors_epoch AFTER INSERT OR UPDATE OR DELETE ON runtime_input_authority FOR EACH ROW EXECUTE FUNCTION native_input_contributor_epoch();
