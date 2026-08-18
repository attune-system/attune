-- Seed an immutable snapshot of the standard public pack index. Appending on
-- upgrades preserves the resolution order of existing administrator-managed
-- indices; a fresh database assigns it position zero.
SET search_path TO attune, public;

-- Query parameters can contain credentials and were accepted before URL
-- hardening. In groups touched by query removal, keep the highest-priority row
-- (position, then id) so administrator ordering and configuration survive.
-- Lower-priority headers are deliberately not combined because conflicting
-- authorization headers cannot be merged safely. Clean-only equivalent rows
-- remain distinct. Every group touched by query removal is disabled for
-- explicit administrator review.
CREATE TEMP TABLE _attune_pack_index_canonical_dedup AS
WITH index_urls AS (
    SELECT
        registry_index.id,
        registry_index.position,
        strpos(registry_index.url, '?') > 0 AS query_tainted,
        CASE
            WHEN authority.value IS NULL THEN split_part(registry_index.url, '?', 1)
            ELSE 'https://'
                || lower(regexp_replace(
                    regexp_replace(
                        regexp_replace(authority.value, '[.](:[0-9]+)$', '\1'),
                        '[.]$',
                        ''
                    ),
                    ':443$',
                    ''
                ))
                || substring(
                    split_part(registry_index.url, '?', 1)
                    FROM length(authority.value) + 9
                )
        END AS canonical_url
    FROM pack_registry_index registry_index
    CROSS JOIN LATERAL (
        SELECT substring(
            split_part(registry_index.url, '?', 1)
            FROM '(?i)^https://([^/?#]+)'
        ) AS value
    ) authority
)
SELECT
    index_urls.*,
    row_number() OVER (
        PARTITION BY canonical_url
        ORDER BY position, id
    ) AS priority,
    bool_or(query_tainted) OVER (
        PARTITION BY canonical_url
    ) AS query_tainted_group
FROM index_urls;

-- This must be a separate statement: data-modifying CTEs do not guarantee
-- delete-before-update execution order for immediate unique constraints.
DELETE FROM pack_registry_index duplicate
USING _attune_pack_index_canonical_dedup ranked
WHERE duplicate.id = ranked.id
  AND ranked.query_tainted_group
  AND ranked.priority > 1;

UPDATE pack_registry_index survivor
SET url = split_part(survivor.url, '?', 1),
    enabled = FALSE
FROM _attune_pack_index_canonical_dedup ranked
WHERE survivor.id = ranked.id
  AND ranked.priority = 1
  AND ranked.query_tainted_group;

DROP TABLE _attune_pack_index_canonical_dedup;

UPDATE pack
SET source_url = split_part(source_url, '?', 1),
    meta = jsonb_set(
        CASE
            WHEN jsonb_typeof(meta) = 'object' THEN meta
            ELSE jsonb_build_object(
                '_attune_legacy_meta',
                COALESCE(meta, 'null'::jsonb)
            )
        END,
        '{_attune_source_query_redacted}',
        'true'::jsonb,
        TRUE
    )
WHERE source_url IS NOT NULL
  AND source_url ~* '^https?://'
  AND strpos(source_url, '?') > 0;

UPDATE audit_event
SET details = jsonb_set(
        jsonb_set(
            details,
            '{source}',
            to_jsonb(split_part(details ->> 'source', '?', 1)),
            FALSE
        ),
        '{source_query_redacted}',
        'true'::jsonb,
        TRUE
    )
WHERE jsonb_typeof(details -> 'source') = 'string'
  AND category = 'pack'
  AND event_type = 'pack.installed'
  AND strpos(details ->> 'source', '?') > 0;

WITH next_position AS (
    SELECT LEAST(COALESCE(MAX(position)::bigint + 1, 0), 2147483647)::integer AS position
    FROM pack_registry_index
)
INSERT INTO pack_registry_index (name, url, position, enabled, headers)
SELECT
    'Attune Standard Pack Index',
    'https://raw.githubusercontent.com/attune-system/index/793aabcc0eb537af7681a386b591de6c4fafd7a1/index.json',
    next_position.position,
    TRUE,
    '{}'::jsonb
FROM next_position
WHERE NOT EXISTS (
    SELECT 1
    FROM pack_registry_index existing
    WHERE regexp_replace(
        existing.url,
        '^https://raw[.]githubusercontent[.]com[.]?(:443)?/',
        'https://raw.githubusercontent.com/',
        'i'
    ) = 'https://raw.githubusercontent.com/attune-system/index/793aabcc0eb537af7681a386b591de6c4fafd7a1/index.json'
)
ON CONFLICT (url) DO NOTHING;
