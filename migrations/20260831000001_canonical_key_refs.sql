-- Derive globally unique key refs from authoritative ownership and a local ref.

ALTER TABLE key ADD COLUMN IF NOT EXISTS local_ref TEXT;
ALTER TABLE key DROP CONSTRAINT IF EXISTS key_ref_canonical;
ALTER TABLE key DROP CONSTRAINT IF EXISTS key_ref_lowercase;
ALTER TABLE key DROP CONSTRAINT IF EXISTS key_ref_format;
ALTER TABLE key DROP CONSTRAINT IF EXISTS key_ref_key;
ALTER TABLE key DISABLE TRIGGER validate_key_owner_trigger;

UPDATE key k
SET owner = i.login
FROM identity i
WHERE k.owner_type = 'identity' AND k.owner_identity = i.id;

UPDATE key k
SET owner = p.ref,
    owner_pack_ref = p.ref
FROM pack p
WHERE k.owner_type = 'pack' AND k.owner_pack = p.id;

UPDATE key k
SET owner = a.ref,
    owner_action_ref = a.ref
FROM action a
WHERE k.owner_type = 'action' AND k.owner_action = a.id;

UPDATE key k
SET owner = s.ref,
    owner_sensor_ref = s.ref
FROM sensor s
WHERE k.owner_type = 'sensor' AND k.owner_sensor = s.id;

CREATE OR REPLACE FUNCTION _legacy_key_local_ref(
    p_owner_type owner_type_enum,
    p_owner_identity_login TEXT,
    p_owner_identity_id BIGINT,
    p_owner_pack_ref TEXT,
    p_owner_action_ref TEXT,
    p_owner_sensor_ref TEXT,
    p_old_ref TEXT
)
RETURNS TEXT AS $$
DECLARE
    owner_prefix TEXT;
    scoped_owner_prefix TEXT;
    numeric_identity_prefix TEXT;
    candidate TEXT;
    sanitized TEXT;
BEGIN
    owner_prefix := CASE p_owner_type
        WHEN 'identity' THEN p_owner_identity_login
        WHEN 'pack' THEN p_owner_pack_ref
        WHEN 'action' THEN p_owner_action_ref
        WHEN 'sensor' THEN p_owner_sensor_ref
        ELSE NULL
    END;
    scoped_owner_prefix := CASE p_owner_type
        WHEN 'system' THEN 'system'
        WHEN 'identity' THEN 'identity.' || p_owner_identity_login
        WHEN 'pack' THEN 'pack.' || p_owner_pack_ref
        WHEN 'action' THEN 'action.' || p_owner_action_ref
        WHEN 'sensor' THEN 'sensor.' || p_owner_sensor_ref
    END;
    numeric_identity_prefix := CASE
        WHEN p_owner_type = 'identity' AND p_owner_identity_id IS NOT NULL
            THEN 'identity.' || p_owner_identity_id::TEXT
        ELSE NULL
    END;
    candidate := CASE
        WHEN scoped_owner_prefix IS NOT NULL
             AND left(p_old_ref, char_length(scoped_owner_prefix) + 1) = scoped_owner_prefix || '.'
            THEN substr(p_old_ref, char_length(scoped_owner_prefix) + 2)
        WHEN numeric_identity_prefix IS NOT NULL
             AND left(p_old_ref, char_length(numeric_identity_prefix) + 1) = numeric_identity_prefix || '.'
            THEN substr(p_old_ref, char_length(numeric_identity_prefix) + 2)
        WHEN owner_prefix IS NOT NULL
             AND left(p_old_ref, char_length(owner_prefix) + 1) = owner_prefix || '.'
            THEN substr(p_old_ref, char_length(owner_prefix) + 2)
        WHEN p_owner_type = 'identity'
             AND p_owner_identity_id IS NOT NULL
             AND left(p_old_ref, char_length(p_owner_identity_id::TEXT) + 1) = p_owner_identity_id::TEXT || '.'
            THEN substr(p_old_ref, char_length(p_owner_identity_id::TEXT) + 2)
        ELSE p_old_ref
    END;

    IF candidate ~ '^[a-z0-9][a-z0-9_-]{0,62}$' THEN
        RETURN candidate;
    END IF;

    sanitized := regexp_replace(candidate, '[^a-z0-9_-]+', '_', 'g');
    sanitized := regexp_replace(sanitized, '^[^a-z0-9]+', '');
    IF sanitized = '' THEN
        sanitized := 'key';
    END IF;
    RETURN left(sanitized, 50) || '_' || left(md5(p_old_ref), 12);
END;
$$ LANGUAGE plpgsql IMMUTABLE;

UPDATE key
SET local_ref = _legacy_key_local_ref(
    owner_type,
    owner,
    owner_identity,
    owner_pack_ref,
    owner_action_ref,
    owner_sensor_ref,
    ref
)
WHERE local_ref IS NULL;

WITH duplicate_groups AS (
    SELECT owner_type, owner, local_ref
    FROM key
    GROUP BY owner_type, owner, local_ref
    HAVING COUNT(*) > 1
)
UPDATE key k
SET local_ref = left(k.local_ref, 50) || '_' || left(md5(k.ref), 12)
FROM duplicate_groups duplicates
WHERE k.owner_type = duplicates.owner_type
  AND k.owner IS NOT DISTINCT FROM duplicates.owner
  AND k.local_ref = duplicates.local_ref;

DROP FUNCTION _legacy_key_local_ref(owner_type_enum, TEXT, BIGINT, TEXT, TEXT, TEXT, TEXT);

CREATE OR REPLACE FUNCTION canonical_key_ref(
    p_owner_type owner_type_enum,
    p_owner_identity_login TEXT,
    p_owner_pack_ref TEXT,
    p_owner_action_ref TEXT,
    p_owner_sensor_ref TEXT,
    p_local_ref TEXT
)
RETURNS TEXT AS $$
BEGIN
    RETURN CASE p_owner_type
        WHEN 'system' THEN 'system.' || p_local_ref
        WHEN 'identity' THEN 'identity.' || p_owner_identity_login || '.' || p_local_ref
        WHEN 'pack' THEN 'pack.' || p_owner_pack_ref || '.' || p_local_ref
        WHEN 'action' THEN 'action.' || p_owner_action_ref || '.' || p_local_ref
        WHEN 'sensor' THEN 'sensor.' || p_owner_sensor_ref || '.' || p_local_ref
    END;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

DO $$
BEGIN
    IF EXISTS (
        SELECT canonical_key_ref(
            owner_type,
            owner,
            owner_pack_ref,
            owner_action_ref,
            owner_sensor_ref,
            local_ref
        )
        FROM key
        GROUP BY 1
        HAVING COUNT(*) > 1
    ) THEN
        RAISE EXCEPTION 'Existing keys cannot be migrated: canonical refs would collide';
    END IF;
END;
$$;

CREATE TEMP TABLE key_ref_migration_map (
    old_ref TEXT PRIMARY KEY,
    new_ref TEXT NOT NULL UNIQUE
) ON COMMIT DROP;

INSERT INTO key_ref_migration_map (old_ref, new_ref)
SELECT
    ref,
    canonical_key_ref(
        owner_type,
        owner,
        owner_pack_ref,
        owner_action_ref,
        owner_sensor_ref,
        local_ref
    )
FROM key
WHERE ref IS DISTINCT FROM canonical_key_ref(
    owner_type,
    owner,
    owner_pack_ref,
    owner_action_ref,
    owner_sensor_ref,
    local_ref
);

CREATE OR REPLACE FUNCTION _mapped_canonical_key_ref(value TEXT)
RETURNS TEXT AS $$
    SELECT COALESCE(
        (SELECT new_ref FROM key_ref_migration_map WHERE old_ref = value),
        value
    );
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION _rewrite_key_ref_array(value JSONB)
RETURNS JSONB AS $$
    SELECT COALESCE(
        jsonb_agg(to_jsonb(_mapped_canonical_key_ref(element #>> '{}'))),
        '[]'::JSONB
    )
    FROM jsonb_array_elements(value) AS elements(element);
$$ LANGUAGE sql STABLE;

UPDATE permission_set
SET grants = (
    SELECT jsonb_agg(
        CASE
            WHEN grant_obj->>'resource' = 'keys'
                 AND jsonb_typeof(grant_obj #> '{constraints,refs}') = 'array'
                THEN jsonb_set(
                    grant_obj,
                    '{constraints,refs}',
                    _rewrite_key_ref_array(grant_obj #> '{constraints,refs}')
                )
            ELSE grant_obj
        END
    )
    FROM jsonb_array_elements(grants) AS grant_entries(grant_obj)
)
WHERE jsonb_typeof(grants) = 'array'
  AND EXISTS (
      SELECT 1
      FROM jsonb_array_elements(grants) AS grant_entries(grant_obj)
      WHERE grant_obj->>'resource' = 'keys'
        AND jsonb_typeof(grant_obj #> '{constraints,refs}') = 'array'
        AND grant_obj #> '{constraints,refs}' IS DISTINCT FROM
            _rewrite_key_ref_array(grant_obj #> '{constraints,refs}')
  );

UPDATE work_queue
SET config = jsonb_set(
    config,
    '{dispatch,concurrency,key_ref}',
    to_jsonb(_mapped_canonical_key_ref(config #>> '{dispatch,concurrency,key_ref}'))
)
WHERE config #>> '{dispatch,concurrency,key_ref}' IS NOT NULL
  AND config #>> '{dispatch,concurrency,key_ref}' IS DISTINCT FROM
      _mapped_canonical_key_ref(config #>> '{dispatch,concurrency,key_ref}');

UPDATE work_queue
SET config = jsonb_set(
    config,
    '{dispatch,batch_size,key_ref}',
    to_jsonb(_mapped_canonical_key_ref(config #>> '{dispatch,batch_size,key_ref}'))
)
WHERE config #>> '{dispatch,batch_size,key_ref}' IS NOT NULL
  AND config #>> '{dispatch,batch_size,key_ref}' IS DISTINCT FROM
      _mapped_canonical_key_ref(config #>> '{dispatch,batch_size,key_ref}');

CREATE OR REPLACE FUNCTION _rewrite_dashboard_key_refs(value JSONB)
RETURNS JSONB AS $$
    SELECT jsonb_set(
        value,
        '{data_sources}',
        COALESCE(
            (
                SELECT jsonb_object_agg(
                    source_id,
                    CASE
                        WHEN source->>'type' = 'key_value'
                             AND source #>> '{params,ref}' IS NOT NULL
                            THEN jsonb_set(
                                source,
                                '{params,ref}',
                                to_jsonb(_mapped_canonical_key_ref(source #>> '{params,ref}'))
                            )
                        ELSE source
                    END
                )
                FROM jsonb_each(value->'data_sources') AS sources(source_id, source)
            ),
            '{}'::JSONB
        )
    );
$$ LANGUAGE sql STABLE;

UPDATE dashboard
SET spec = _rewrite_dashboard_key_refs(spec)
WHERE jsonb_typeof(spec->'data_sources') = 'object'
  AND spec IS DISTINCT FROM _rewrite_dashboard_key_refs(spec);

UPDATE dashboard_version
SET spec = _rewrite_dashboard_key_refs(spec)
WHERE jsonb_typeof(spec->'data_sources') = 'object'
  AND spec IS DISTINCT FROM _rewrite_dashboard_key_refs(spec);

UPDATE key SET ref = canonical_key_ref(
    owner_type,
    owner,
    owner_pack_ref,
    owner_action_ref,
    owner_sensor_ref,
    local_ref
);

DROP FUNCTION _rewrite_dashboard_key_refs(JSONB);
DROP FUNCTION _rewrite_key_ref_array(JSONB);
DROP FUNCTION _mapped_canonical_key_ref(TEXT);

ALTER TABLE key ALTER COLUMN local_ref SET NOT NULL;
ALTER TABLE key ADD CONSTRAINT key_ref_key UNIQUE (ref);
ALTER TABLE key DROP CONSTRAINT IF EXISTS key_local_ref_format;
ALTER TABLE key ADD CONSTRAINT key_local_ref_format
    CHECK (local_ref ~ '^[a-z0-9][a-z0-9_-]{0,62}$');
ALTER TABLE key DROP CONSTRAINT IF EXISTS key_ref_canonical;
ALTER TABLE key ADD CONSTRAINT key_ref_canonical CHECK (
    ref = canonical_key_ref(
        owner_type,
        owner,
        owner_pack_ref,
        owner_action_ref,
        owner_sensor_ref,
        local_ref
    )
);

CREATE OR REPLACE FUNCTION validate_key_owner()
RETURNS TRIGGER AS $$
DECLARE
    owner_count INTEGER := 0;
BEGIN
    IF TG_OP = 'UPDATE' AND (
        NEW.owner_type IS DISTINCT FROM OLD.owner_type
        OR NEW.owner_identity IS DISTINCT FROM OLD.owner_identity
        OR NEW.owner_pack IS DISTINCT FROM OLD.owner_pack
        OR NEW.owner_action IS DISTINCT FROM OLD.owner_action
        OR NEW.owner_sensor IS DISTINCT FROM OLD.owner_sensor
        OR NEW.local_ref IS DISTINCT FROM OLD.local_ref
    ) THEN
        RAISE EXCEPTION 'Key owner and local_ref cannot be changed';
    END IF;

    IF NEW.owner_identity IS NOT NULL THEN owner_count := owner_count + 1; END IF;
    IF NEW.owner_pack IS NOT NULL THEN owner_count := owner_count + 1; END IF;
    IF NEW.owner_action IS NOT NULL THEN owner_count := owner_count + 1; END IF;
    IF NEW.owner_sensor IS NOT NULL THEN owner_count := owner_count + 1; END IF;

    IF NEW.owner_type = 'system' THEN
        IF owner_count > 0 THEN
            RAISE EXCEPTION 'System owner cannot have specific owner fields set';
        END IF;
        NEW.owner := 'system';
        NEW.owner_pack_ref := NULL;
        NEW.owner_action_ref := NULL;
        NEW.owner_sensor_ref := NULL;
    ELSIF owner_count != 1 THEN
        RAISE EXCEPTION 'Exactly one owner field must be set for owner_type %', NEW.owner_type;
    ELSIF NEW.owner_type = 'identity' THEN
        IF NEW.owner_identity IS NULL THEN
            RAISE EXCEPTION 'owner_identity must be set for owner_type identity';
        END IF;
        SELECT login INTO STRICT NEW.owner FROM identity WHERE id = NEW.owner_identity;
        NEW.owner_pack_ref := NULL;
        NEW.owner_action_ref := NULL;
        NEW.owner_sensor_ref := NULL;
    ELSIF NEW.owner_type = 'pack' THEN
        IF NEW.owner_pack IS NULL THEN
            RAISE EXCEPTION 'owner_pack must be set for owner_type pack';
        END IF;
        SELECT ref INTO STRICT NEW.owner_pack_ref FROM pack WHERE id = NEW.owner_pack;
        NEW.owner := NEW.owner_pack_ref;
        NEW.owner_action_ref := NULL;
        NEW.owner_sensor_ref := NULL;
    ELSIF NEW.owner_type = 'action' THEN
        IF NEW.owner_action IS NULL THEN
            RAISE EXCEPTION 'owner_action must be set for owner_type action';
        END IF;
        SELECT ref INTO STRICT NEW.owner_action_ref FROM action WHERE id = NEW.owner_action;
        NEW.owner := NEW.owner_action_ref;
        NEW.owner_pack_ref := NULL;
        NEW.owner_sensor_ref := NULL;
    ELSIF NEW.owner_type = 'sensor' THEN
        IF NEW.owner_sensor IS NULL THEN
            RAISE EXCEPTION 'owner_sensor must be set for owner_type sensor';
        END IF;
        SELECT ref INTO STRICT NEW.owner_sensor_ref FROM sensor WHERE id = NEW.owner_sensor;
        NEW.owner := NEW.owner_sensor_ref;
        NEW.owner_pack_ref := NULL;
        NEW.owner_action_ref := NULL;
    END IF;

    NEW.ref := canonical_key_ref(
        NEW.owner_type,
        NEW.owner,
        NEW.owner_pack_ref,
        NEW.owner_action_ref,
        NEW.owner_sensor_ref,
        NEW.local_ref
    );
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP FUNCTION IF EXISTS canonical_key_ref(owner_type_enum, BIGINT, TEXT, TEXT, TEXT, TEXT);
ALTER TABLE key ENABLE TRIGGER validate_key_owner_trigger;

COMMENT ON COLUMN key.ref IS 'Server-generated canonical key reference';
COMMENT ON COLUMN key.local_ref IS 'Dot-free key identifier within the owner scope';
