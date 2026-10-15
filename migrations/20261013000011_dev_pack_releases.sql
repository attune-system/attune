-- A development version may be republished only under the server's opt-in
-- policy. Every distinct digest still has its own immutable release row.
ALTER TABLE pack_release
    DROP CONSTRAINT pack_release_pack_version_unique;

ALTER TABLE pack_release
    ADD CONSTRAINT pack_release_pack_version_digest_unique UNIQUE (pack, version, digest);

CREATE UNIQUE INDEX pack_release_non_dev_version_unique
    ON pack_release (pack, version)
    WHERE version !~ '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-dev$';
