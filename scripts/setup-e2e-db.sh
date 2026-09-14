#!/bin/bash
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
DB_HOST="${DB_HOST:-localhost}"
DB_PORT="${DB_PORT:-5432}"
DB_USER="${DB_USER:-postgres}"
DB_PASSWORD="${DB_PASSWORD:-postgres}"
DB_NAME="attune_e2e"

echo -e "${GREEN}=== Attune E2E Database Setup ===${NC}\n"

# Check if PostgreSQL is running
echo -e "${YELLOW}→${NC} Checking PostgreSQL connection..."
if ! PGPASSWORD=$DB_PASSWORD psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d postgres -c '\q' 2>/dev/null; then
    echo -e "${RED}✗${NC} Cannot connect to PostgreSQL at $DB_HOST:$DB_PORT"
    echo "  Please ensure PostgreSQL is running and credentials are correct."
    exit 1
fi
echo -e "${GREEN}✓${NC} PostgreSQL is running\n"

# Drop existing E2E database if it exists
echo -e "${YELLOW}→${NC} Checking for existing E2E database..."
if PGPASSWORD=$DB_PASSWORD psql -h $DB_HOST -p $DB_PORT -U $DB_USER -lqt | cut -d \| -f 1 | grep -qw $DB_NAME; then
    echo -e "${YELLOW}!${NC} Found existing database '$DB_NAME', dropping it..."

    # Force terminate all connections and drop in single transaction
    PGPASSWORD=$DB_PASSWORD psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d postgres <<EOF
        SELECT pg_terminate_backend(pid)
        FROM pg_stat_activity
        WHERE datname = '$DB_NAME' AND pid <> pg_backend_pid();

        DROP DATABASE IF EXISTS $DB_NAME;
EOF
    echo -e "${GREEN}✓${NC} Dropped existing database"
fi

# Create E2E database
echo -e "${YELLOW}→${NC} Creating E2E database '$DB_NAME'..."
PGPASSWORD=$DB_PASSWORD psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d postgres -c "CREATE DATABASE $DB_NAME;" 2>&1
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} Database created\n"
else
    echo -e "${RED}✗${NC} Failed to create database"
    exit 1
fi

# Create attune schema and set search_path
echo -e "${YELLOW}→${NC} Creating attune schema..."
PGPASSWORD=$DB_PASSWORD psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME <<EOF
CREATE SCHEMA IF NOT EXISTS attune;
ALTER DATABASE $DB_NAME SET search_path TO attune, public;
EOF

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} Schema created and search_path configured\n"
else
    echo -e "${RED}✗${NC} Failed to create schema"
    exit 1
fi

# Run migrations (they will use the attune schema via search_path)
echo -e "${YELLOW}→${NC} Running database migrations..."
DATABASE_URL="postgresql://$DB_USER:$DB_PASSWORD@$DB_HOST:$DB_PORT/$DB_NAME" sqlx migrate run --source ./migrations

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} Migrations completed successfully\n"
else
    echo -e "${RED}✗${NC} Migration failed"
    exit 1
fi

# Seed default runtimes
echo -e "${YELLOW}→${NC} Seeding default runtimes..."
PGPASSWORD=$DB_PASSWORD psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -f ./scripts/seed_runtimes.sql > /dev/null

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} Runtimes seeded successfully\n"
else
    echo -e "${RED}✗${NC} Runtime seeding failed"
    exit 1
fi

# Verify database schema
echo -e "${YELLOW}→${NC} Verifying database schema..."
TABLES=$(PGPASSWORD=$DB_PASSWORD psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -t -c "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'attune';")
TABLES=$(echo $TABLES | tr -d ' ')

if [ "$TABLES" -gt 0 ]; then
    echo -e "${GREEN}✓${NC} Found $TABLES tables in 'attune' schema"
else
    echo -e "${RED}✗${NC} No tables found in 'attune' schema"
    exit 1
fi

# List all tables
echo -e "\n${YELLOW}Tables in attune schema:${NC}"
PGPASSWORD=$DB_PASSWORD psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -c "SELECT table_name FROM information_schema.tables WHERE table_schema = 'attune' ORDER BY table_name;"

# Create default test user
echo -e "\n${YELLOW}→${NC} Creating default test user..."

# Generate a fresh password hash
echo -e "${YELLOW}  Generating password hash...${NC}"
HASH=$(cd crates/common && cargo run --example hash_password TestPass123! 2>/dev/null | tail -1)

if [ -z "$HASH" ]; then
    echo -e "${RED}✗${NC} Failed to generate password hash"
    exit 1
fi

PGPASSWORD=$DB_PASSWORD psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME <<EOF
INSERT INTO attune.identity (login, display_name, password_hash, attributes)
VALUES (
    'test@attune.local',
    'E2E Test User',
    '$HASH',
    jsonb_build_object(
        'email', 'test@attune.local',
        'is_active', true,
        'is_system', false,
        'type', 'user'
    )
)
ON CONFLICT (login) DO UPDATE SET
    password_hash = EXCLUDED.password_hash,
    attributes = EXCLUDED.attributes;
EOF

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} Test user created (or already exists)"
else
    echo -e "${YELLOW}!${NC} Could not create test user (might already exist)"
fi

echo -e "\n${GREEN}=== E2E Database Setup Complete ===${NC}"
echo -e "\nDatabase Details:"
echo -e "  • Name: ${GREEN}$DB_NAME${NC}"
echo -e "  • Host: ${GREEN}$DB_HOST:$DB_PORT${NC}"
echo -e "  • User: ${GREEN}$DB_USER${NC}"
echo -e "  • Endpoint: ${GREEN}postgresql://$DB_HOST:$DB_PORT/$DB_NAME${NC}"
echo -e "\nTest User:"
echo -e "  • Login:    ${GREEN}test@attune.local${NC}"
echo -e "  • Password: ${GREEN}TestPass123!${NC}"
echo -e "\nNext Steps:"
echo -e "  1. Run ${YELLOW}./scripts/start-e2e-services.sh${NC} to start all services"
echo -e "  2. Run ${YELLOW}cargo test --test integration${NC} to execute E2E tests"
echo ""
