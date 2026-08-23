SET search_path TO attune, public;

-- Releases before the standard marker used three URLs for the managed index.
-- Keep the highest-priority administrator state unless a row is already marked.
CREATE TEMP TABLE _attune_standard_index_candidates AS
WITH candidates AS (
    SELECT
        registry_index.id,
        registry_index.is_standard,
        registry_index.position
    FROM pack_registry_index registry_index
    WHERE registry_index.is_standard
       OR regexp_replace(
            registry_index.url,
            '^https://raw[.]githubusercontent[.]com[.]?(:443)?/',
            'https://raw.githubusercontent.com/',
            'i'
        ) IN (
            'https://raw.githubusercontent.com/attune-system/index/main/index.json',
            'https://raw.githubusercontent.com/attune-system/index/793aabcc0eb537af7681a386b591de6c4fafd7a1/index.json',
            'https://raw.githubusercontent.com/attune-system/index/c9e48439677847797d056efb94ba1c855e188df9/index.json'
        )
)
SELECT
    candidates.id,
    row_number() OVER (
        ORDER BY candidates.is_standard DESC, candidates.position, candidates.id
    ) AS priority
FROM candidates;

DELETE FROM pack_registry_index duplicate
USING _attune_standard_index_candidates candidate
WHERE duplicate.id = candidate.id
  AND candidate.priority > 1;

UPDATE pack_registry_index survivor
SET is_standard = TRUE
FROM _attune_standard_index_candidates candidate
WHERE survivor.id = candidate.id
  AND candidate.priority = 1
  AND NOT survivor.is_standard;

DROP TABLE _attune_standard_index_candidates;

-- An absent row after migration 26 means an administrator deleted it. Record
-- that state so later seed runs do not recreate the managed index.
INSERT INTO standard_pack_index_seed_state (id)
VALUES (1)
ON CONFLICT (id) DO NOTHING;
