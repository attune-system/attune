ALTER TABLE pack_registry_index
    ADD COLUMN source_headers JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(source_headers) = 'object');

COMMENT ON COLUMN pack_registry_index.source_headers IS
    'Exact HTTPS source origins mapped to encrypted HTTP headers for registry archives and Git clones';
