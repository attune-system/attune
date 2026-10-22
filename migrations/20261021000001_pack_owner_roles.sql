-- Ownership declarations do not assign identity roles or permission sets.
ALTER TABLE pack ADD COLUMN owner_roles TEXT[] NOT NULL DEFAULT '{}';

CREATE FUNCTION valid_pack_owner_roles(roles TEXT[]) RETURNS BOOLEAN
LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT cardinality(roles) <= 64
       AND array_position(roles, NULL) IS NULL
       AND NOT EXISTS (
           SELECT 1 FROM unnest(roles) role
           WHERE char_length(role) NOT BETWEEN 1 AND 255
              OR role !~ '[^[:space:]]' OR role ~ '[[:cntrl:]]'
       )
       AND cardinality(roles) = (SELECT COUNT(DISTINCT role) FROM unnest(roles) role)
$$;

ALTER TABLE pack ADD CONSTRAINT pack_owner_roles_valid CHECK (valid_pack_owner_roles(owner_roles));
