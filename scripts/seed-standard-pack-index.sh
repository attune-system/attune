#!/bin/sh

set -eu

DB_HOST=${DB_HOST:-localhost}
DB_PORT=${DB_PORT:-5432}
DB_USER=${DB_USER:-attune}
DB_PASSWORD=${DB_PASSWORD:-attune}
DB_NAME=${DB_NAME:-attune}
DATABASE_URL=${DATABASE_URL:-${ATTUNE__DATABASE__URL:-}}
DEFAULT_STANDARD_INDEX_REF=4c87ca62a4313f7e9646a50c44ab6b2b530e5f43
STANDARD_INDEX_REF=${ATTUNE_STANDARD_PACK_INDEX_REF:-$DEFAULT_STANDARD_INDEX_REF}
STANDARD_INDEX_TIMEOUT=${ATTUNE_STANDARD_PACK_INDEX_TIMEOUT:-30}

usage() {
    printf 'Usage: %s [--ref <40-character commit SHA>]\n' "$0" >&2
}

if [ "${1:-}" = "--ref" ]; then
    [ "$#" -eq 2 ] || { usage; exit 2; }
    STANDARD_INDEX_REF=$2
elif [ "$#" -ne 0 ]; then
    usage
    exit 2
fi

case "$STANDARD_INDEX_REF" in
    *[!0-9a-f]*)
        printf 'Standard index ref must be a 40-character lowercase commit SHA\n' >&2
        exit 2
        ;;
esac
[ "${#STANDARD_INDEX_REF}" -eq 40 ] || {
    printf 'Standard index ref must be a 40-character lowercase commit SHA\n' >&2
    exit 2
}

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/attune-standard-index.XXXXXX")
trap 'rm -rf -- "$work_dir"' EXIT HUP INT TERM

export PGPASSWORD="$DB_PASSWORD"
export PGOPTIONS="${PGOPTIONS:-} -c search_path=attune,public"

run_psql() {
    if [ -n "$DATABASE_URL" ]; then
        PGDATABASE="$DATABASE_URL" psql -X "$@"
    else
        psql -X -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" "$@"
    fi
}

index_url="https://raw.githubusercontent.com/attune-system/index/${STANDARD_INDEX_REF}/index.json"

seed_action=$(run_psql -v ON_ERROR_STOP=1 -v index_url="$index_url" -Atq <<'EOSQL'
SELECT CASE
    WHEN EXISTS (
        SELECT 1
        FROM pack_registry_index
        WHERE is_standard
          AND regexp_replace(
              url,
              '^https://raw[.]githubusercontent[.]com[.]?(:443)?/',
              'https://raw.githubusercontent.com/',
              'i'
          ) ~ '^https://raw[.]githubusercontent[.]com/attune-system/index/([0-9a-f]{40}|main)/index[.]json$'
    ) THEN 'update'
    WHEN EXISTS (SELECT 1 FROM pack_registry_index WHERE is_standard) THEN 'custom'
    WHEN EXISTS (SELECT 1 FROM standard_pack_index_seed_state WHERE id = 1) THEN 'deleted'
    ELSE 'create'
END;
EOSQL
)

if [ "$seed_action" = "deleted" ]; then
    printf 'Standard pack index remains deleted; skipping seed\n'
    exit 0
fi
if [ "$seed_action" = "custom" ]; then
    printf 'Standard pack index has an administrator-managed URL; skipping seed update\n'
    exit 0
fi

fetch() {
    wget -q -T "$STANDARD_INDEX_TIMEOUT" -O "$1" "$2"
}

fetch "$work_dir/index.json" "$index_url"

valid=$(run_psql -v ON_ERROR_STOP=1 -Atq <<EOSQL
\set QUIET 1
BEGIN;
CREATE TEMP TABLE standard_index_validation (encoded TEXT NOT NULL) ON COMMIT DROP;
\copy standard_index_validation (encoded) FROM PROGRAM 'base64 "$work_dir/index.json" | tr -d "\\n"'
SELECT jsonb_typeof(convert_from(decode(encoded, 'base64'), 'UTF8')::jsonb -> 'packs') = 'array'
       AND convert_from(decode(encoded, 'base64'), 'UTF8')::jsonb ? 'registry_name' AS valid
FROM standard_index_validation \gset
COMMIT;
\echo :valid
EOSQL
)
[ "$valid" = "t" ] || {
    printf 'Standard index does not contain the expected registry JSON structure\n' >&2
    exit 1
}

run_psql -v ON_ERROR_STOP=1 -v index_url="$index_url" <<'EOSQL'
BEGIN;
SELECT pg_advisory_xact_lock(hashtextextended('standard_pack_index_seed', 0));

UPDATE pack_registry_index
SET url = :'index_url', updated = NOW()
WHERE is_standard
  AND regexp_replace(
      url,
      '^https://raw[.]githubusercontent[.]com[.]?(:443)?/',
      'https://raw.githubusercontent.com/',
      'i'
  ) ~ '^https://raw[.]githubusercontent[.]com/attune-system/index/([0-9a-f]{40}|main)/index[.]json$';

INSERT INTO pack_registry_index (name, url, position, enabled, is_standard, headers)
SELECT
    'Attune Standard Pack Index',
    :'index_url',
    (
        SELECT LEAST(COALESCE(MAX(position)::bigint + 1, 0), 2147483647)::integer
        FROM pack_registry_index
    ),
    TRUE,
    TRUE,
    '{}'::jsonb
WHERE NOT EXISTS (SELECT 1 FROM pack_registry_index WHERE is_standard)
  AND NOT EXISTS (SELECT 1 FROM standard_pack_index_seed_state WHERE id = 1);

INSERT INTO standard_pack_index_seed_state (id)
SELECT 1
WHERE EXISTS (SELECT 1 FROM pack_registry_index WHERE is_standard)
ON CONFLICT (id) DO NOTHING;
COMMIT;
EOSQL

printf 'Standard pack index is pinned to %s\n' "$STANDARD_INDEX_REF"
