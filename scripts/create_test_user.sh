#!/bin/bash
# Create or reset test admin user for local development
# Login: admin, Password: admin

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Default values
DB_NAME="${ATTUNE_DB_NAME:-attune}"
DB_USER="${ATTUNE_DB_USER:-postgres}"
DB_HOST="${ATTUNE_DB_HOST:-localhost}"
DB_PORT="${ATTUNE_DB_PORT:-5432}"
DB_PASSWORD="${ATTUNE_DB_PASSWORD:-postgres}"

# Admin user credentials
ADMIN_LOGIN="${1:-admin}"
ADMIN_PASSWORD="${2:-admin}"
ADMIN_DISPLAY_NAME="${3:-Administrator}"

print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

print_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

# Check PostgreSQL connection
check_postgres() {
    if ! PGPASSWORD="$DB_PASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -c '\q' 2>/dev/null; then
        print_error "Cannot connect to database '$DB_NAME' at $DB_HOST:$DB_PORT"
        exit 1
    fi
}

# Generate Argon2id hash for password
hash_password() {
    local password="$1"

    # Check if we can use Python with argon2-cffi
    if command -v python3 &> /dev/null; then
        python3 -c "
import sys
try:
    from argon2 import PasswordHasher
    ph = PasswordHasher()
    print(ph.hash('$password'))
    sys.exit(0)
except ImportError:
    sys.exit(1)
" 2>/dev/null && return 0
    fi

    # Fallback: Use a pre-generated hash for 'admin' password
    if [ "$password" = "admin" ]; then
        # This is the Argon2id hash for password 'admin'
        # Generated with: argon2-cffi default parameters
        echo '$argon2id$v=19$m=19456,t=2,p=1$9Z0VWE8xbJMGPJ8kQ3qRmA$iGBqNEdvklvGLJH8TdUv6u+5c8WU8P9v7UzxQXmkFsE'
        return 0
    fi

    print_error "Cannot hash password - Python with argon2-cffi not available"
    print_error "Please install with: pip install argon2-cffi"
    exit 1
}

# Create or update admin user
create_or_update_user() {
    local login="$1"
    local password="$2"
    local display_name="$3"

    print_info "Generating password hash..."
    local password_hash
    password_hash=$(hash_password "$password")

    if [ -z "$password_hash" ]; then
        print_error "Failed to generate password hash"
        exit 1
    fi

    print_info "Checking if user '$login' exists..."

    local user_exists
    user_exists=$(PGPASSWORD="$DB_PASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -tAc \
        "SELECT COUNT(*) FROM identity WHERE login='$login'")

    if [ "$user_exists" -gt 0 ]; then
        print_warn "User '$login' already exists. Updating password..."
        PGPASSWORD="$DB_PASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" <<EOF
UPDATE identity
SET password_hash = '$password_hash',
    display_name = '$display_name',
    updated = NOW()
WHERE login = '$login';
EOF
        print_info "User '$login' password updated successfully!"
    else
        print_info "Creating new user '$login'..."
        PGPASSWORD="$DB_PASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" <<EOF
INSERT INTO identity (login, display_name, password_hash, attributes)
VALUES ('$login', '$display_name', '$password_hash', '{}');
EOF
        print_info "User '$login' created successfully!"
    fi

    echo ""
    print_info "======================================"
    print_info "Test User Credentials:"
    print_info "  Login:    $login"
    print_info "  Password: [configured, ${#password} characters]"
    print_info "======================================"
    echo ""
}

show_help() {
    cat << EOF
Create or Reset Test Admin User

Usage: $0 [LOGIN] [PASSWORD] [DISPLAY_NAME]

Arguments:
    LOGIN         User login name (default: admin)
    PASSWORD      User password (default: admin)
    DISPLAY_NAME  User display name (default: Administrator)

Environment Variables:
    ATTUNE_DB_NAME      Database name (default: attune)
    ATTUNE_DB_USER      Database user (default: postgres)
    ATTUNE_DB_HOST      Database host (default: localhost)
    ATTUNE_DB_PORT      Database port (default: 5432)
    ATTUNE_DB_PASSWORD  Database password (default: postgres)

Examples:
    # Create/reset default admin user (admin/admin)
    $0

    # Create/reset with custom credentials
    $0 myuser mypassword "My User"

    # Use custom database connection
    ATTUNE_DB_PASSWORD=secret $0

Note: If Python with argon2-cffi is not available, only the default
      'admin' password can be used (pre-hashed).
EOF
}

# Main script
main() {
    if [ "$1" = "-h" ] || [ "$1" = "--help" ]; then
        show_help
        exit 0
    fi

    print_info "Attune Test User Setup"
    print_info "======================"
    print_info "Database: $DB_NAME"
    print_info "Host: $DB_HOST:$DB_PORT"
    echo ""

    check_postgres
    create_or_update_user "$ADMIN_LOGIN" "$ADMIN_PASSWORD" "$ADMIN_DISPLAY_NAME"

    print_info "You can now login to the API:"
    echo ""
    echo "  curl -X POST http://localhost:8080/auth/login \\"
    echo "    -H 'Content-Type: application/json' \\"
    echo '    -d "{\"login\":\"<login>\",\"password\":\"<password>\"}"'
}

main "$@"
