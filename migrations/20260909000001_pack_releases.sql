CREATE TABLE pack_release (
    id BIGSERIAL PRIMARY KEY,
    pack BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    pack_ref TEXT NOT NULL,
    version TEXT NOT NULL,
    digest TEXT NOT NULL,
    archive_path TEXT NOT NULL,
    content_path TEXT NOT NULL,
    archive_size BIGINT NOT NULL CHECK (archive_size >= 0),
    manifest JSONB NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT pack_release_pack_version_unique UNIQUE (pack, version),
    CONSTRAINT pack_release_pack_id_unique UNIQUE (pack, id),
    CONSTRAINT pack_release_digest_sha256 CHECK (digest ~ '^[0-9a-f]{64}$'),
    CONSTRAINT pack_release_manifest_object CHECK (jsonb_typeof(manifest) = 'object')
);

CREATE INDEX idx_pack_release_pack_created ON pack_release(pack, created DESC);
CREATE INDEX idx_pack_release_digest ON pack_release(digest);

ALTER TABLE pack
    ADD COLUMN active_release BIGINT,
    ADD CONSTRAINT pack_active_release_same_pack
        FOREIGN KEY (id, active_release)
        REFERENCES pack_release(pack, id)
        DEFERRABLE INITIALLY DEFERRED;

COMMENT ON TABLE pack_release IS
    'Immutable, digest-verified filesystem releases for packs';
COMMENT ON COLUMN pack_release.manifest IS
    'Executable file metadata for the exact archived release';
COMMENT ON COLUMN pack.active_release IS
    'Release whose metadata is projected into the active component tables';
