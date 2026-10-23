-- Named types are pack-managed projections. Executions must use immutable
-- release revisions and self-contained bindings, never this mutable schema.
CREATE TABLE schema_type (
    id BIGSERIAL PRIMARY KEY,
    ref TEXT NOT NULL UNIQUE CHECK (ref ~ '^[a-z][a-z0-9_-]*\.[a-z][a-z0-9_-]*$'),
    pack BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    pack_ref TEXT NOT NULL,
    label VARCHAR(255) NOT NULL CHECK (length(btrim(label)) > 0),
    description TEXT,
    schema JSONB NOT NULL CHECK (jsonb_typeof(schema) IN ('object', 'boolean')),
    reference_visibility action_reference_visibility_enum NOT NULL DEFAULT 'public',
    reference_allowed_pack_refs TEXT[] NOT NULL DEFAULT '{}',
    catalog_revision INTEGER CHECK (catalog_revision IS NULL),
    management_origin management_origin GENERATED ALWAYS AS ('pack'::management_origin) STORED,
    managed_release BIGINT,
    retired_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (split_part(ref, '.', 1) = pack_ref),
    CHECK (reference_visibility = 'restricted' OR cardinality(reference_allowed_pack_refs) = 0),
    FOREIGN KEY (pack, managed_release) REFERENCES pack_release(pack, id) DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX idx_schema_type_pack ON schema_type (pack);
CREATE INDEX idx_schema_type_active ON schema_type (pack, ref) WHERE retired_at IS NULL;
CREATE TRIGGER set_component_managed_release BEFORE INSERT OR UPDATE ON schema_type
    FOR EACH ROW EXECUTE FUNCTION set_component_managed_release('pack');

CREATE TABLE schema_type_revision (
    id BIGSERIAL PRIMARY KEY,
    release BIGINT NOT NULL REFERENCES pack_release(id) ON DELETE CASCADE,
    type_ref TEXT NOT NULL,
    definition JSONB NOT NULL CHECK (jsonb_typeof(definition) = 'object'),
    content_digest TEXT NOT NULL CHECK (content_digest ~ '^[0-9a-f]{64}$'),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (release, type_ref)
);

CREATE TABLE schema_type_release_snapshot (
    release BIGINT PRIMARY KEY REFERENCES pack_release(id) ON DELETE CASCADE,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE FUNCTION guard_schema_type_revision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'named type revision bodies are immutable';
END $$;
CREATE TRIGGER guard_schema_type_revision BEFORE UPDATE ON schema_type_revision
    FOR EACH ROW EXECUTE FUNCTION guard_schema_type_revision();

-- Active authored definitions, not runtime bindings. Updating/retiring a type
-- must not strand active consumers. Historical bindings retain their own bodies.
CREATE TABLE schema_type_reference (
    source_pack BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL,
    source_ref TEXT NOT NULL,
    schema_field TEXT NOT NULL,
    target_ref TEXT NOT NULL,
    PRIMARY KEY (source_pack, source_kind, source_ref, schema_field, target_ref)
);
CREATE INDEX idx_schema_type_reference_target ON schema_type_reference (target_ref, source_pack);
