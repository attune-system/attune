-- Input mutation revokes Run/selected-input delivery permanently. It does not
-- classify outputs or revoke retained Capture merely because inputs changed.
-- Epochs/tombstones intentionally retain no source FK.
ALTER TABLE workload_source_epoch DROP CONSTRAINT workload_source_epoch_kind_check;
ALTER TABLE workload_source_epoch ADD CONSTRAINT workload_source_epoch_kind_check
    CHECK (kind IN ('owner','execution','worker','workload','sensor','admission','install','image','engine','execution_input'));

CREATE FUNCTION revoke_workload_execution_input() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE affected BIGINT; previous_execution BIGINT; next_execution BIGINT;
BEGIN
    IF TG_TABLE_NAME='execution' THEN
        IF ROW(OLD.config,OLD.env_vars) IS DISTINCT FROM ROW(NEW.config,NEW.env_vars) THEN
            PERFORM bump_workload_source_epoch('execution_input',OLD.id);
        END IF;
    ELSE
        IF TG_OP='UPDATE' THEN
            IF ROW(OLD.entity_type,OLD.entity_id,OLD.json_path,OLD.source_kind,
                OLD.source_ref,OLD.encrypted_value,OLD.encryption_key_hash)
                IS NOT DISTINCT FROM ROW(NEW.entity_type,NEW.entity_id,NEW.json_path,NEW.source_kind,
                NEW.source_ref,NEW.encrypted_value,NEW.encryption_key_hash) THEN
                RETURN NULL;
            END IF;
        END IF;
        IF TG_OP<>'INSERT' THEN
            IF OLD.entity_type='execution_config' THEN previous_execution=OLD.entity_id; END IF;
        END IF;
        IF TG_OP<>'DELETE' THEN
            IF NEW.entity_type='execution_config' THEN next_execution=NEW.entity_id; END IF;
        END IF;
        FOR affected IN SELECT DISTINCT id FROM unnest(ARRAY[previous_execution,next_execution]) AS source(id)
            WHERE id IS NOT NULL ORDER BY id
        LOOP PERFORM bump_workload_source_epoch('execution_input',affected); END LOOP;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER workload_execution_selected_input_epoch AFTER UPDATE ON execution
    FOR EACH ROW EXECUTE FUNCTION revoke_workload_execution_input();
CREATE TRIGGER workload_execution_input_origins_epoch AFTER INSERT OR UPDATE OR DELETE ON execution_secret_value
    FOR EACH ROW EXECUTE FUNCTION revoke_workload_execution_input();
