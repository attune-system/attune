-- Preserve the canonical set default as well as per-grant overrides. Expanding
-- inheritance into effective grants must not rewrite a later YAML export's
-- default for newly authored grants.
CREATE TABLE permission_set_authoring_definition (
    permission_set BIGINT PRIMARY KEY REFERENCES permission_set(id) ON DELETE CASCADE,
    definition JSONB NOT NULL CHECK (jsonb_typeof(definition) = 'object')
);
