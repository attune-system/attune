-- Initial input materialization uses the existing config column. The general
-- execution update DTO remains unable to mutate accepted config. Existing
-- record_execution_history does not track config; record only its digest here.
CREATE FUNCTION record_execution_config_history() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO execution_history(time,operation,entity_id,entity_ref,changed_fields,old_values,new_values)
    VALUES (clock_timestamp(),'UPDATE',NEW.id,NEW.action_ref,ARRAY['config'],
        jsonb_build_object('config',_jsonb_digest_summary(OLD.config)),
        jsonb_build_object('config',_jsonb_digest_summary(NEW.config)));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER record_execution_config_history
    AFTER UPDATE OF config ON execution FOR EACH ROW
    WHEN (OLD.config IS DISTINCT FROM NEW.config)
    EXECUTE FUNCTION record_execution_config_history();
