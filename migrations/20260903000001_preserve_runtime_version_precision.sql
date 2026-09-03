UPDATE runtime_version
SET version_minor = CASE
        WHEN split_part(regexp_replace(version, '^[vV]', ''), '.', 2) = '' THEN NULL
        ELSE version_minor
    END,
    version_patch = CASE
        WHEN split_part(split_part(regexp_replace(version, '^[vV]', ''), '-', 1), '.', 3) = '' THEN NULL
        ELSE version_patch
    END
WHERE version_minor IS NOT NULL OR version_patch IS NOT NULL;
