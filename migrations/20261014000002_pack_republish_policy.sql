-- Republishing is governed by the server's off/on/suffix policy, not a fixed
-- development suffix in the database. Retain UNIQUE (pack, version, digest)
-- so every distinct publication remains an immutable, independently pinned row.
DROP INDEX pack_release_non_dev_version_unique;
