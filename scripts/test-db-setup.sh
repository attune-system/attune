#!/bin/bash
# Test Database Setup Script
# This script helps set up and manage the test database for Attune

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
DB_NAME="attune_test"
DB_USER="${POSTGRES_USER:-postgres}"
DB_HOST="${POSTGRES_HOST:-localhost}"
DB_PORT="${POSTGRES_PORT:-5432}"

# Handle password: use env var if set, otherwise prompt once
if [ -z "$POSTGRES_PASSWORD" ]; then
    if [ -z "$PGPASSWORD" ]; then
        # Prompt for password once
        read -sp "Enter PostgreSQL password for user $DB_USER: " DB_PASSWORD
        echo ""
        export PGPASSWORD="$DB_PASSWORD"
    fi
    # else PGPASSWORD is already set, use it
else
    # POSTGRES_PASSWORD was provided, use it
    export PGPASSWORD="$POSTGRES_PASSWORD"
fi

DB_URL="postgresql://${DB_USER}:${PGPASSWORD}@${DB_HOST}:${DB_PORT}/${DB_NAME}"

# Functions
print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_postgres() {
    print_info "Checking PostgreSQL connection..."
    if ! pg_isready -h "$DB_HOST" -p "$DB_PORT" > /dev/null 2>&1; then
        print_error "PostgreSQL is not running or not accessible at ${DB_HOST}:${DB_PORT}"
        exit 1
    fi
    print_info "PostgreSQL is running"
}

db_exists() {
    psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -lqt | cut -d \| -f 1 | grep -qw "$DB_NAME"
}

create_database() {
    print_info "Creating test database: $DB_NAME"
    if db_exists; then
        print_warn "Database $DB_NAME already exists"
    else
        createdb -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" "$DB_NAME"
        print_info "Database $DB_NAME created successfully"
    fi
}

drop_database() {
    print_info "Dropping test database: $DB_NAME"
    if db_exists; then
        dropdb -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" "$DB_NAME"
        print_info "Database $DB_NAME dropped successfully"
    else
        print_warn "Database $DB_NAME does not exist"
    fi
}

run_migrations() {
    print_info "Running migrations on test database..."
    DATABASE_URL="$DB_URL" sqlx migrate run
    print_info "Migrations completed successfully"
}

clean_database() {
    print_info "Cleaning test database..."

    PSQL_CMD="psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME"

    # Disable triggers to avoid constraint issues
    $PSQL_CMD -c "SET session_replication_role = replica;"

    # Delete in reverse dependency order
    $PSQL_CMD -c "DELETE FROM executions;"
    $PSQL_CMD -c "DELETE FROM inquiries;"
    $PSQL_CMD -c "DELETE FROM enforcements;"
    $PSQL_CMD -c "DELETE FROM events;"
    $PSQL_CMD -c "DELETE FROM rules;"
    $PSQL_CMD -c "DELETE FROM triggers;"
    $PSQL_CMD -c "DELETE FROM notifications;"
    $PSQL_CMD -c "DELETE FROM keys;"
    $PSQL_CMD -c "DELETE FROM identities;"
    $PSQL_CMD -c "DELETE FROM workers;"
    $PSQL_CMD -c "DELETE FROM runtimes;"
    $PSQL_CMD -c "DELETE FROM actions;"
    $PSQL_CMD -c "DELETE FROM packs;"

    # Re-enable triggers
    $PSQL_CMD -c "SET session_replication_role = DEFAULT;"

    print_info "Database cleaned successfully"
}

verify_schema() {
    print_info "Verifying database schema..."

    PSQL_CMD="psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -t"

    # Check for essential tables in attune schema
    TABLES=("pack" "action" "runtime" "worker" "trigger" "rule" "event" "enforcement" "execution" "inquiry" "identity" "key" "notification")

    for table in "${TABLES[@]}"; do
        if $PSQL_CMD -c "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'attune' AND table_name = '$table');" | grep -q 't'; then
            echo -e "  ${GREEN}✓${NC} Table 'attune.$table' exists"
        else
            echo -e "  ${RED}✗${NC} Table 'attune.$table' missing"
            return 1
        fi
    done

    print_info "Schema verification passed"
}

show_status() {
    print_info "Test Database Status"
    echo "  Database: $DB_NAME"
    echo "  Host: $DB_HOST:$DB_PORT"
    echo "  User: $DB_USER"
    echo "  Endpoint: postgresql://${DB_HOST}:${DB_PORT}/${DB_NAME}"
    echo ""

    if db_exists; then
        echo -e "  Status: ${GREEN}EXISTS${NC}"

        PSQL_CMD="psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -t -c"

        # Count tables
        TABLE_COUNT=$($PSQL_CMD "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'attune' AND table_type = 'BASE TABLE';" | tr -d ' ')
        echo "  Tables: $TABLE_COUNT"

        # Count migrations
        MIGRATION_COUNT=$($PSQL_CMD "SELECT COUNT(*) FROM _sqlx_migrations WHERE success = true;" 2>/dev/null | tr -d ' ' || echo "0")
        echo "  Migrations: $MIGRATION_COUNT"

        # Count records in each table
        echo ""
        echo "  Record counts:"
        for table in pack action runtime worker trigger rule event enforcement execution inquiry identity key notification; do
            COUNT=$($PSQL_CMD "SELECT COUNT(*) FROM attune.$table;" 2>/dev/null | tr -d ' ' || echo "0")
            printf "    %-15s %s\n" "$table:" "$COUNT"
        done
    else
        echo -e "  Status: ${RED}DOES NOT EXIST${NC}"
    fi
}

show_help() {
    cat << EOF
Test Database Setup Script for Attune

Usage: $0 [command]

Commands:
    setup       Create database and run migrations (default)
    create      Create the test database
    drop        Drop the test database
    reset       Drop, create, and migrate the database
    migrate     Run migrations on existing database
    clean       Delete all data from tables
    verify      Verify database schema
    status      Show database status and record counts
    help        Show this help message

Environment Variables:
    POSTGRES_USER       PostgreSQL user (default: postgres)
    POSTGRES_PASSWORD   PostgreSQL password (prompted if not set)
    PGPASSWORD          PostgreSQL password (alternative to POSTGRES_PASSWORD)
    POSTGRES_HOST       PostgreSQL host (default: localhost)
    POSTGRES_PORT       PostgreSQL port (default: 5432)

Examples:
    $0 setup                # Create and setup test database
    $0 reset                # Reset test database
    $0 clean                # Clean all data
    $0 status               # Show database status

EOF
}

# Main
case "${1:-setup}" in
    setup)
        check_postgres
        create_database
        run_migrations
        verify_schema
        print_info "Test database setup complete!"
        ;;
    create)
        check_postgres
        create_database
        ;;
    drop)
        check_postgres
        drop_database
        ;;
    reset)
        check_postgres
        drop_database
        create_database
        run_migrations
        verify_schema
        print_info "Test database reset complete!"
        ;;
    migrate)
        check_postgres
        run_migrations
        ;;
    clean)
        check_postgres
        if ! db_exists; then
            print_error "Database $DB_NAME does not exist"
            exit 1
        fi
        clean_database
        ;;
    verify)
        check_postgres
        if ! db_exists; then
            print_error "Database $DB_NAME does not exist"
            exit 1
        fi
        verify_schema
        ;;
    status)
        check_postgres
        show_status
        ;;
    help|--help|-h)
        show_help
        ;;
    *)
        print_error "Unknown command: $1"
        show_help
        exit 1
        ;;
esac
