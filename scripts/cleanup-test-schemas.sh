#!/bin/bash
set -e

# Cleanup orphaned test schemas
# Run this periodically in development or CI to remove leftover test schemas

# Default to attune_test database, can be overridden with DATABASE_URL env var
DATABASE_URL="${DATABASE_URL:-postgresql://postgres:postgres@localhost:5432/attune_test}"

sanitize_database_url() {
    local at_signs=${1//[^@]/}
    if [[ ${#at_signs} -gt 1 || $1 == *$'\n'* || $1 == *$'\r'* ]]; then
        printf '%s\n' '<database-url configured>'
        return
    fi
    printf '%s\n' "$1" | sed -E 's#(://).*@#\1#; s#[?#].*$##'
}

echo "============================================="
echo "Attune Test Schema Cleanup Utility"
echo "============================================="
echo "Target database: $(sanitize_database_url "$DATABASE_URL")"
echo ""

# Check if psql is available
if ! command -v psql &> /dev/null; then
    echo "ERROR: psql command not found. Please install PostgreSQL client."
    exit 1
fi

# Count schemas before cleanup
BEFORE_COUNT=$(psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM pg_namespace WHERE nspname LIKE 'test_%';" 2>/dev/null || echo "0")
BEFORE_COUNT=$(echo "$BEFORE_COUNT" | xargs) # trim whitespace

echo "Found $BEFORE_COUNT test schema(s) to clean up"
echo ""

if [ "$BEFORE_COUNT" = "0" ]; then
    echo "No test schemas to clean up. Exiting."
    exit 0
fi

# Confirm cleanup in interactive mode (skip if CI or --force flag)
if [ -t 0 ] && [ "$1" != "--force" ] && [ "$CI" != "true" ]; then
    read -p "Do you want to proceed with cleanup? (y/N) " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Cleanup cancelled."
        exit 0
    fi
fi

echo "Starting cleanup..."
echo ""

# Process schemas in batches to avoid PostgreSQL shared memory issues
BATCH_SIZE=50
TOTAL_DROPPED=0
BATCH_NUM=1

while true; do
    # Get next batch of schemas
    SCHEMAS=$(psql "$DATABASE_URL" -t -c "SELECT nspname FROM pg_namespace WHERE nspname LIKE 'test_%' ORDER BY nspname LIMIT $BATCH_SIZE;" 2>/dev/null | xargs)

    if [ -z "$SCHEMAS" ]; then
        echo "No more schemas to clean up"
        break
    fi

    echo "Processing batch $BATCH_NUM (up to $BATCH_SIZE schemas)..."

    # Drop schemas in this batch
    BATCH_DROPPED=$(psql "$DATABASE_URL" -t <<EOF 2>&1
DO \$\$
DECLARE
    schema_name TEXT;
    schema_count INTEGER := 0;
BEGIN
    FOR schema_name IN
        SELECT nspname
        FROM pg_namespace
        WHERE nspname LIKE 'test_%'
        ORDER BY nspname
        LIMIT $BATCH_SIZE
    LOOP
        BEGIN
            EXECUTE format('DROP SCHEMA IF EXISTS %I CASCADE', schema_name);
            schema_count := schema_count + 1;
        EXCEPTION WHEN OTHERS THEN
            RAISE WARNING 'Failed to drop schema %: %', schema_name, SQLERRM;
        END;
    END LOOP;

    RAISE NOTICE 'Batch complete: % schemas dropped', schema_count;
    -- Return the count
    PERFORM schema_count;
END \$\$;
EOF
)

    echo "  Batch $BATCH_NUM complete"
    BATCH_NUM=$((BATCH_NUM + 1))
    TOTAL_DROPPED=$((TOTAL_DROPPED + BATCH_SIZE))

    # Brief pause to let PostgreSQL clean up
    sleep 0.5
done

echo ""
echo "============================================"
echo "Cleanup Summary"
echo "============================================"
echo "Total batches processed: $((BATCH_NUM - 1))"
echo "Estimated schemas processed: ~$TOTAL_DROPPED"

EXIT_CODE=$?

echo ""
if [ $EXIT_CODE -eq 0 ]; then
    echo "✓ Cleanup completed successfully"
else
    echo "✗ Cleanup failed with exit code $EXIT_CODE"
    exit $EXIT_CODE
fi

# Verify cleanup
AFTER_COUNT=$(psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM pg_namespace WHERE nspname LIKE 'test_%';" 2>/dev/null || echo "0")
AFTER_COUNT=$(echo "$AFTER_COUNT" | xargs) # trim whitespace

echo "Remaining test schemas: $AFTER_COUNT"
echo ""

if [ "$AFTER_COUNT" != "0" ]; then
    echo "WARNING: Some test schemas were not cleaned up. Please investigate."
    exit 1
fi

echo "All test schemas have been removed."
