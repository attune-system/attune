#!/bin/bash
# Create Test User Account
# This script creates a test user in the Attune database

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Default values
DB_NAME="${ATTUNE_DB_NAME:-attune}"
DB_USER="${ATTUNE_DB_USER:-postgres}"
DB_HOST="${ATTUNE_DB_HOST:-localhost}"
DB_PORT="${ATTUNE_DB_PORT:-5432}"
DB_PASSWORD="${ATTUNE_DB_PASSWORD:-postgres}"

# Test user defaults
TEST_LOGIN="${TEST_LOGIN:-test@attune.local}"
TEST_DISPLAY_NAME="${TEST_DISPLAY_NAME:-Test User}"
TEST_PASSWORD="${TEST_PASSWORD:-TestPass123!}"

# Pre-generated hash for default password "TestPass123!"
# If you change TEST_PASSWORD, you need to regenerate this with:
#   cargo run --example hash_password "YourPassword"
DEFAULT_PASSWORD_HASH='$argon2id$v=19$m=19456,t=2,p=1$F0UlGNd21LBXF7TWmpD93w$F65DKRjPU6japrzYv3ZcddnMFCtjVIBDWIkiLbkqt2I'

echo -e "${BLUE}╔════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║        Attune Test User Setup                  ║${NC}"
echo -e "${BLUE}╚════════════════════════════════════════════════╝${NC}"
echo ""

# Check PostgreSQL connection
echo -e "${YELLOW}→${NC} Checking database connection..."
if ! PGPASSWORD="$DB_PASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -c '\q' 2>/dev/null; then
    echo -e "${RED}✗${NC} Cannot connect to database $DB_NAME"
    echo ""
    echo "Please check:"
    echo "  • Database exists: $DB_NAME"
    echo "  • PostgreSQL is running on $DB_HOST:$DB_PORT"
    echo "  • Credentials are correct"
    exit 1
fi
echo -e "${GREEN}✓${NC} Database connection successful"
echo ""

# Determine password hash to use
PASSWORD_HASH="$DEFAULT_PASSWORD_HASH"

# If custom password, generate hash
if [ "$TEST_PASSWORD" != "TestPass123!" ]; then
    echo -e "${YELLOW}→${NC} Generating password hash for custom password..."
    cd "$(dirname "$0")/.."

    PASSWORD_HASH=$(cargo run --quiet --example hash_password "$TEST_PASSWORD" 2>/dev/null || echo "")

    if [ -z "$PASSWORD_HASH" ]; then
        echo -e "${RED}✗${NC} Failed to generate password hash"
        echo ""
        echo "Please ensure Rust toolchain is installed, or use default password."
        echo "To manually hash a password:"
        echo "  cargo run --example hash_password \"YourPassword\""
        exit 1
    fi

    echo -e "${GREEN}✓${NC} Password hash generated"
    echo ""
else
    echo -e "${GREEN}✓${NC} Using default password hash"
    echo ""
fi

# Check if user already exists
echo -e "${YELLOW}→${NC} Checking if user exists..."
USER_EXISTS=$(PGPASSWORD="$DB_PASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -tAc "SELECT COUNT(*) FROM identity WHERE login='$TEST_LOGIN'")

if [ "$USER_EXISTS" -gt 0 ]; then
    echo -e "${YELLOW}!${NC} User '$TEST_LOGIN' already exists"
    echo ""
    read -p "Do you want to update the password? (y/N): " -n 1 -r
    echo ""
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${YELLOW}→${NC} Updating user password..."
        PGPASSWORD="$DB_PASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" << EOF
UPDATE identity
SET password_hash = '$PASSWORD_HASH',
    display_name = '$TEST_DISPLAY_NAME',
    updated = NOW()
WHERE login = '$TEST_LOGIN';
EOF
        echo -e "${GREEN}✓${NC} User password updated"
    else
        echo -e "${BLUE}ℹ${NC} User not modified"
        exit 0
    fi
else
    # Create new user
    echo -e "${YELLOW}→${NC} Creating user..."
    PGPASSWORD="$DB_PASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" << EOF
INSERT INTO identity (login, display_name, password_hash, attributes)
VALUES ('$TEST_LOGIN', '$TEST_DISPLAY_NAME', '$PASSWORD_HASH', '{}');
EOF
    echo -e "${GREEN}✓${NC} User created"
fi

echo ""
echo -e "${GREEN}╔════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║  Test User Setup Complete!                     ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${BLUE}User Credentials:${NC}"
echo -e "  Login:    ${YELLOW}$TEST_LOGIN${NC}"
echo -e "  Password: ${YELLOW}[configured, ${#TEST_PASSWORD} characters]${NC}"
echo -e "  Name:     ${YELLOW}$TEST_DISPLAY_NAME${NC}"
echo ""
echo -e "${BLUE}Database:${NC}"
echo -e "  Host:     ${YELLOW}$DB_HOST:$DB_PORT${NC}"
echo -e "  Database: ${YELLOW}$DB_NAME${NC}"
echo ""
echo -e "${BLUE}Test Login:${NC}"
echo -e "  ${YELLOW}curl -X POST http://localhost:8080/auth/login \\${NC}"
echo -e "  ${YELLOW}  -H 'Content-Type: application/json' \\${NC}"
echo -e '  -d "{\"login\":\"<login>\",\"password\":\"<password>\"}"'
echo ""
echo -e "${BLUE}Custom User:${NC}"
echo -e "  You can create a custom user by setting environment variables:"
echo -e "  ${YELLOW}TEST_LOGIN=myuser@example.com TEST_PASSWORD=MyPass123! ./scripts/create-test-user.sh${NC}"
echo ""
echo -e "${BLUE}Generate Password Hash:${NC}"
echo -e "  To generate a hash for a custom password:"
echo -e "  ${YELLOW}cargo run --example hash_password \"YourPassword\"${NC}"
echo ""
