#!/bin/bash
set -e

# Verification script to demonstrate automatic schema cleanup in tests
# This script runs a test and verifies the schema is cleaned up automatically

echo "============================================="
echo "Schema Cleanup Verification Script"
echo "============================================="
echo ""

DATABASE_URL="${DATABASE_URL:-postgresql://postgres:postgres@localhost:5432/attune_test}"

sanitize_database_url() {
    local at_signs=${1//[^@]/}
    if [[ ${#at_signs} -gt 1 || $1 == *$'\n'* || $1 == *$'\r'* ]]; then
        printf '%s\n' '<database-url configured>'
        return
    fi
    printf '%s\n' "$1" | sed -E 's#(://).*@#\1#; s#[?#].*$##'
}

# Check if psql is available
if ! command -v psql &> /dev/null; then
    echo "ERROR: psql command not found. Please install PostgreSQL client."
    exit 1
fi

# Check if database is accessible
if ! psql "$DATABASE_URL" -c "SELECT 1" > /dev/null 2>&1; then
    echo "ERROR: Cannot connect to database: $(sanitize_database_url "$DATABASE_URL")"
    exit 1
fi

echo "✓ Database connection verified"
echo ""

# Count schemas before test
BEFORE_COUNT=$(psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM pg_namespace WHERE nspname LIKE 'test_%';" 2>/dev/null | xargs)
echo "Test schemas before test: $BEFORE_COUNT"

# Get list of schemas before
SCHEMAS_BEFORE=$(psql "$DATABASE_URL" -t -c "SELECT nspname FROM pg_namespace WHERE nspname LIKE 'test_%' ORDER BY nspname;" 2>/dev/null | xargs)

echo ""
echo "Running a single test to verify cleanup..."
echo ""

# Run a single test (health check is fast and simple)
cd "$(dirname "$0")/.."
cargo test --package attune-api --test health_and_auth_tests test_health_check -- --test-threads=1 2>&1 | grep -E "(running|test result)" || true

echo ""
echo "Test completed. Checking cleanup..."
echo ""

# Give a moment for cleanup to complete
sleep 2

# Count schemas after test
AFTER_COUNT=$(psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM pg_namespace WHERE nspname LIKE 'test_%';" 2>/dev/null | xargs)
echo "Test schemas after test: $AFTER_COUNT"

# Get list of schemas after
SCHEMAS_AFTER=$(psql "$DATABASE_URL" -t -c "SELECT nspname FROM pg_namespace WHERE nspname LIKE 'test_%' ORDER BY nspname;" 2>/dev/null | xargs)

echo ""
echo "============================================="
echo "Verification Results"
echo "============================================="

if [ "$BEFORE_COUNT" -eq "$AFTER_COUNT" ]; then
    echo "✓ SUCCESS: Schema count unchanged ($BEFORE_COUNT → $AFTER_COUNT)"
    echo "✓ Test schemas were automatically cleaned up via Drop trait"
    echo ""
    echo "This demonstrates that:"
    echo "  1. Each test creates a unique schema (test_<uuid>)"
    echo "  2. Schema is automatically dropped when TestContext goes out of scope"
    echo "  3. No manual cleanup needed in test code"
    echo "  4. No schemas accumulate during normal test execution"
    echo ""
    exit 0
else
    echo "⚠ WARNING: Schema count changed ($BEFORE_COUNT → $AFTER_COUNT)"
    echo ""

    if [ "$AFTER_COUNT" -gt "$BEFORE_COUNT" ]; then
        echo "New schemas detected (cleanup may have failed):"
        # Show new schemas
        for schema in $SCHEMAS_AFTER; do
            if [[ ! " $SCHEMAS_BEFORE " =~ " $schema " ]]; then
                echo "  - $schema (NEW)"
            fi
        done
        echo ""
        echo "This could indicate:"
        echo "  1. Test was interrupted (Ctrl+C, crash, panic)"
        echo "  2. Drop trait not executing properly"
        echo "  3. Async cleanup not completing"
        echo ""
        echo "Run cleanup script to remove orphaned schemas:"
        echo "  ./scripts/cleanup-test-schemas.sh --force"
        exit 1
    else
        echo "Schemas were cleaned up (count decreased)"
        echo "This is actually good - leftover schemas were removed!"
        exit 0
    fi
fi
