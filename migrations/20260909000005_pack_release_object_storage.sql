ALTER TABLE pack_release
    ADD COLUMN object_key TEXT,
    ADD COLUMN provider_version TEXT,
    ALTER COLUMN archive_path DROP NOT NULL;

ALTER TABLE pack_release
    ADD CONSTRAINT pack_release_object_pointer_complete CHECK (
        (object_key IS NULL AND provider_version IS NULL)
        OR (object_key IS NOT NULL AND provider_version IS NOT NULL)
    );

COMMENT ON COLUMN pack_release.object_key IS
    'Content-addressed BlobStore key for the immutable release archive';
COMMENT ON COLUMN pack_release.provider_version IS
    'Opaque exact object version used for every archive read';
