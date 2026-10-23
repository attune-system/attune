-- Normalize immutable entity-origin edges so source retention does not scan
-- encrypted values or guess dependencies from serialized strings.
CREATE TABLE runtime_secret_origin_reference (
    secret_value BIGINT NOT NULL REFERENCES execution_secret_value(id) ON DELETE CASCADE,
    source_entity TEXT NOT NULL,
    source_id BIGINT NOT NULL CHECK (source_id > 0),
    PRIMARY KEY (secret_value, source_entity, source_id)
);
CREATE INDEX runtime_secret_origin_source_idx ON runtime_secret_origin_reference (source_entity, source_id);

CREATE FUNCTION runtime_secret_entity_origins(source_kind TEXT, source_ref TEXT)
RETURNS TABLE (source_entity TEXT, source_id BIGINT) LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE body JSONB;
BEGIN
    IF source_kind <> 'provenance' OR source_ref IS NULL OR NOT (source_ref IS JSON OBJECT) THEN RETURN; END IF;
    body := source_ref::jsonb;
    IF jsonb_typeof(body->'origins') <> 'array' THEN RETURN; END IF;
    RETURN QUERY SELECT DISTINCT origin->>'entity_type',
        CASE WHEN jsonb_typeof(origin->'entity_id') = 'number'
            AND origin->>'entity_id' ~ '^[1-9][0-9]*$'
            AND (length(origin->>'entity_id') < 19 OR
                (length(origin->>'entity_id') = 19 AND origin->>'entity_id' <= '9223372036854775807'))
            THEN (origin->>'entity_id')::bigint END
        FROM jsonb_array_elements(body->'origins') origin
        WHERE origin->>'kind' = 'entity' AND jsonb_typeof(origin->'entity_type') = 'string';
END $$;

CREATE FUNCTION maintain_runtime_secret_origin_references() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM runtime_secret_origin_reference WHERE secret_value = NEW.id;
    INSERT INTO runtime_secret_origin_reference (secret_value, source_entity, source_id)
        SELECT NEW.id, origin.source_entity, origin.source_id
        FROM runtime_secret_entity_origins(NEW.source_kind, NEW.source_ref) origin
        WHERE origin.source_id IS NOT NULL ON CONFLICT DO NOTHING;
    RETURN NEW;
END $$;
CREATE TRIGGER maintain_runtime_secret_origin_references AFTER INSERT OR UPDATE OF source_kind, source_ref ON execution_secret_value
    FOR EACH ROW EXECUTE FUNCTION maintain_runtime_secret_origin_references();

INSERT INTO runtime_secret_origin_reference (secret_value, source_entity, source_id)
    SELECT secret.id, origin.source_entity, origin.source_id
    FROM execution_secret_value secret
    CROSS JOIN LATERAL runtime_secret_entity_origins(secret.source_kind, secret.source_ref) origin
    WHERE origin.source_id IS NOT NULL ON CONFLICT DO NOTHING;
