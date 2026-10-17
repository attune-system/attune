-- Keep authoring data in permission_set.grants. Every effective reader uses this
-- one function, including direct/role expansion and SQL snapshot producers.
-- Platform/unmanaged policies retain their declared platform scope. Managed
-- definitions default to structural pack ownership, not a pack-ref prefix.
CREATE FUNCTION effective_permission_set_grants(authored JSONB, managing_pack BIGINT)
RETURNS JSONB LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE effective JSONB;
BEGIN
    IF managing_pack <= 0 AND EXISTS (SELECT 1 FROM jsonb_array_elements(authored) item WHERE item->>'scope' = 'pack') THEN
        RAISE EXCEPTION 'Pack-local policy requires an actual managing pack';
    END IF;
    IF EXISTS (SELECT 1 FROM jsonb_array_elements(authored) item
        WHERE item->>'scope' IS NOT NULL AND item->>'scope' NOT IN ('pack','external')) THEN
        RAISE EXCEPTION 'Invalid authored permission scope';
    END IF;
    SELECT COALESCE(jsonb_agg(
        CASE WHEN managing_pack > 0 AND COALESCE(item->>'scope', 'pack') = 'pack'
        THEN (item - 'scope') || jsonb_build_object('constraints',
            (COALESCE(NULLIF(item->'constraints', 'null'::jsonb), '{}'::jsonb) - 'managing_pack_id')
            || jsonb_build_object('managing_pack_id', managing_pack))
        ELSE (item - 'scope') || CASE WHEN item ? 'constraints'
            THEN jsonb_build_object('constraints', (item->'constraints') - 'managing_pack_id')
            ELSE '{}'::jsonb END
        END ORDER BY ordinal
    ), '[]'::jsonb)
    INTO effective FROM jsonb_array_elements(authored) WITH ORDINALITY AS grants(item, ordinal);
    RETURN effective;
END;
$$;

CREATE FUNCTION validated_authored_permission_set_grants(authored JSONB, managing_pack BIGINT)
RETURNS JSONB LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
    PERFORM effective_permission_set_grants(authored, managing_pack);
    RETURN authored;
END;
$$;
