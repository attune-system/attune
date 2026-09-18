#!/usr/bin/env bash
set -euo pipefail

# Cleanup databases and legacy schemas owned by one explicit test-run identity.
# This utility refuses broad prefixes because they may belong to a neighboring run.
FORCE=false
RUN_ID="${ATTUNE_TEST_RUN_ID:-}"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --force) FORCE=true; shift ;;
        --run-id) RUN_ID="${2:-}"; shift 2 ;;
        *) echo "Usage: $0 --run-id <id> [--force]" >&2; exit 2 ;;
    esac
done
if [[ ! "$RUN_ID" =~ ^[a-z0-9][a-z0-9-]{0,19}$ ]]; then
    echo "ERROR: --run-id or ATTUNE_TEST_RUN_ID must be 1-20 lowercase ASCII letters/digits with optional non-leading '-'" >&2
    exit 2
fi
RUN_TOKEN="${RUN_ID//-/_}"
SCHEMA_PREFIX="test_${RUN_TOKEN}_"
DATABASE_PREFIX="attune_db_${RUN_TOKEN}_"
MIGRATION_PREFIX="attune_migration_${RUN_TOKEN}_"
TEMPLATE_PREFIX="attune_tpl_${RUN_TOKEN}_"

# Default to the dedicated test database, never the development database.
DATABASE_URL="${DATABASE_URL:-postgresql://postgres:postgres@localhost:5432/attune_test}"

sanitize_database_url() {
    local at_signs=${1//[^@]/}
    if [[ ${#at_signs} -gt 1 || $1 == *$'\n'* || $1 == *$'\r'* ]]; then
        printf '%s\n' '<database-url configured>'
        return
    fi
    printf '%s\n' "$1" | sed -E 's#(://).*@#\1#; s#[?#].*$##'
}

psql_admin() {
    psql -X "$DATABASE_URL" -v ON_ERROR_STOP=1 -qAt "$@"
}

count_schemas() {
    psql_admin -c "SELECT count(*) FROM pg_catalog.pg_namespace WHERE left(nspname, ${#SCHEMA_PREFIX}) = '${SCHEMA_PREFIX}';"
}

count_databases() {
    psql_admin -c "SELECT count(*) FROM pg_catalog.pg_database WHERE left(datname, ${#DATABASE_PREFIX}) = '${DATABASE_PREFIX}';"
}

count_migration_databases() {
    psql_admin -c "SELECT count(*) FROM pg_catalog.pg_database WHERE left(datname, ${#MIGRATION_PREFIX}) = '${MIGRATION_PREFIX}';"
}

count_templates() {
    psql_admin -c "SELECT count(*) FROM pg_catalog.pg_database WHERE left(datname, ${#TEMPLATE_PREFIX}) = '${TEMPLATE_PREFIX}';"
}

require_count() {
    if [[ ! "$1" =~ ^[0-9]+$ ]]; then
        echo "ERROR: PostgreSQL returned an invalid schema count" >&2
        exit 1
    fi
}

echo "============================================="
echo "Attune Test Schema Cleanup Utility"
echo "============================================="
echo "Target database: $(sanitize_database_url "$DATABASE_URL")"
echo ""

if ! command -v psql &> /dev/null; then
    echo "ERROR: psql command not found. Please install PostgreSQL client." >&2
    exit 1
fi

BEFORE_COUNT="$(count_schemas)"
DATABASE_COUNT="$(count_databases)"
MIGRATION_COUNT="$(count_migration_databases)"
TEMPLATE_COUNT="$(count_templates)"
require_count "$BEFORE_COUNT"
require_count "$DATABASE_COUNT"
require_count "$MIGRATION_COUNT"
require_count "$TEMPLATE_COUNT"
echo "Found $DATABASE_COUNT test database(s), $MIGRATION_COUNT migration database(s), $TEMPLATE_COUNT template database(s), and $BEFORE_COUNT legacy test schema(s)"
echo ""

if (( BEFORE_COUNT == 0 && DATABASE_COUNT == 0 && MIGRATION_COUNT == 0 && TEMPLATE_COUNT == 0 )); then
    echo "No run-owned test resources to clean up. Exiting."
    exit 0
fi

if [[ -t 0 && "$FORCE" != true && "${CI:-}" != "true" ]]; then
    read -r -p "Do you want to proceed with cleanup? (y/N) " -n 1 REPLY
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Cleanup cancelled."
        exit 0
    fi
fi

echo "Starting cleanup..."
DROP_STATEMENTS="$(psql_admin -c \
    "SELECT format('DROP DATABASE %I WITH (FORCE)', datname) FROM pg_catalog.pg_database WHERE left(datname, ${#DATABASE_PREFIX}) = '${DATABASE_PREFIX}' ORDER BY datname;")"
while IFS= read -r drop_statement; do
    [[ -z "$drop_statement" ]] && continue
    psql_admin -c "$drop_statement" </dev/null
done <<< "$DROP_STATEMENTS"
DROP_STATEMENTS="$(psql_admin -c \
    "SELECT format('DROP DATABASE %I WITH (FORCE)', datname) FROM pg_catalog.pg_database WHERE left(datname, ${#MIGRATION_PREFIX}) = '${MIGRATION_PREFIX}' ORDER BY datname;")"
while IFS= read -r drop_statement; do
    [[ -z "$drop_statement" ]] && continue
    psql_admin -c "$drop_statement" </dev/null
done <<< "$DROP_STATEMENTS"
TEMPLATE_DATABASES="$(psql_admin -c \
    "SELECT datname FROM pg_catalog.pg_database WHERE left(datname, ${#TEMPLATE_PREFIX}) = '${TEMPLATE_PREFIX}' ORDER BY datname;")"
while IFS= read -r database_name; do
    [[ -z "$database_name" ]] && continue
    psql_admin -c "ALTER DATABASE \"$database_name\" IS_TEMPLATE false;" </dev/null
    psql_admin -c "DROP DATABASE \"$database_name\" WITH (FORCE);" </dev/null
done <<< "$TEMPLATE_DATABASES"

BATCH_SIZE=50
TOTAL_DROPPED=0
BATCH_NUM=1
CURRENT_COUNT=$BEFORE_COUNT

while (( CURRENT_COUNT > 0 )); do
    echo "Processing batch $BATCH_NUM (up to $BATCH_SIZE schemas)..."
    psql_admin <<EOF
DO \$\$
DECLARE
    schema_name TEXT;
BEGIN
    FOR schema_name IN
        SELECT nspname
        FROM pg_catalog.pg_namespace
        WHERE left(nspname, ${#SCHEMA_PREFIX}) = '${SCHEMA_PREFIX}'
        ORDER BY nspname
        LIMIT $BATCH_SIZE
    LOOP
        EXECUTE format('DROP SCHEMA %I CASCADE', schema_name);
    END LOOP;
END \$\$;
EOF

    NEXT_COUNT="$(count_schemas)"
    require_count "$NEXT_COUNT"
    if (( NEXT_COUNT >= CURRENT_COUNT )); then
        echo "ERROR: schema cleanup made no progress ($CURRENT_COUNT remain)" >&2
        exit 1
    fi
    TOTAL_DROPPED=$((TOTAL_DROPPED + CURRENT_COUNT - NEXT_COUNT))
    CURRENT_COUNT=$NEXT_COUNT
    BATCH_NUM=$((BATCH_NUM + 1))
done

REMAINING_DATABASES="$(count_databases)"
REMAINING_MIGRATIONS="$(count_migration_databases)"
REMAINING_TEMPLATES="$(count_templates)"
require_count "$REMAINING_DATABASES"
require_count "$REMAINING_MIGRATIONS"
require_count "$REMAINING_TEMPLATES"
if ((REMAINING_DATABASES != 0 || REMAINING_MIGRATIONS != 0 || REMAINING_TEMPLATES != 0)); then
    echo "ERROR: database cleanup left run-owned resources" >&2
    exit 1
fi

echo ""
echo "============================================"
echo "Cleanup Summary"
echo "============================================"
echo "Total batches processed: $((BATCH_NUM - 1))"
echo "Databases dropped: $DATABASE_COUNT"
echo "Migration databases dropped: $MIGRATION_COUNT"
echo "Templates dropped: $TEMPLATE_COUNT"
echo "Legacy schemas dropped: $TOTAL_DROPPED"
echo "Remaining legacy test schemas: $CURRENT_COUNT"
echo "All run-owned test database resources have been removed."
