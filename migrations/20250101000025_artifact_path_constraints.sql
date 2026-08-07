-- Keep persisted artifact refs and file paths safe for path derivation even if
-- a writer bypasses application validation.
ALTER TABLE artifact
    ADD CONSTRAINT artifact_ref_path_safe CHECK (
        ref <> ''
        AND ref = btrim(ref)
        AND ref !~ '(^\.|\.{2}|[/:\\]|\.$)'
    ) NOT VALID;

ALTER TABLE artifact_version
    ADD CONSTRAINT artifact_version_file_path_safe CHECK (
        file_path IS NULL
        OR (
            file_path <> ''
            AND file_path !~ '(^/|\\|(^|/)\.\.?(/|$)|//|/$|:)'
        )
    ) NOT VALID;

-- NOT VALID keeps this migration upgrade-safe when historical rows predate the
-- path grammar. PostgreSQL still enforces both checks for new or updated rows.
-- Operators can preflight legacy violations without modifying data:
--
-- SELECT id, ref FROM artifact
-- WHERE NOT (ref <> '' AND ref = btrim(ref)
--            AND ref !~ '(^\.|\.{2}|[/:\\]|\.$)');
--
-- SELECT id, file_path FROM artifact_version
-- WHERE file_path IS NOT NULL
--   AND NOT (file_path <> ''
--            AND file_path !~ '(^/|\\|(^|/)\.\.?(/|$)|//|/$|:)');
--
-- After remediation, validate explicitly with:
-- ALTER TABLE artifact VALIDATE CONSTRAINT artifact_ref_path_safe;
-- ALTER TABLE artifact_version
--     VALIDATE CONSTRAINT artifact_version_file_path_safe;
--
-- Do not add uniqueness here. Historical refs and paths may already collide,
-- and authorization treats ambiguous paths as non-mutable instead of deleting
-- or silently rewriting persisted data during migration.
