#!/bin/bash
# Wrapper script for loading the core pack into Attune database
# Usage: ./scripts/load-core-pack.sh [options]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Default values
DATABASE_URL="${DATABASE_URL:-postgresql://postgres:postgres@localhost:5432/attune}"
PACKS_DIR="${ATTUNE_PACKS_DIR:-$PROJECT_ROOT/packs}"
PYTHON_BIN="python3"

sanitize_database_url() {
    local at_signs=${1//[^@]/}
    if [[ ${#at_signs} -gt 1 || $1 == *$'\n'* || $1 == *$'\r'* ]]; then
        printf '%s\n' '<database-url configured>'
        return
    fi
    printf '%s\n' "$1" | sed -E 's#(://).*@#\1#; s#[?#].*$##'
}

# Function to print colored messages
info() {
    echo -e "${BLUE}ℹ${NC} $1"
}

success() {
    echo -e "${GREEN}✓${NC} $1"
}

error() {
    echo -e "${RED}✗${NC} $1"
}

warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

# Parse command line arguments
DRY_RUN=false
VERBOSE=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --database-url)
            DATABASE_URL="$2"
            shift 2
            ;;
        --pack-dir)
            PACKS_DIR="$2"
            shift 2
            ;;
        --python)
            PYTHON_BIN="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -v|--verbose)
            VERBOSE=true
            shift
            ;;
        -h|--help)
            cat << EOF
Usage: $0 [options]

Load the Attune core pack into the database.

Options:
    --database-url URL    PostgreSQL connection string
                         (default: postgresql://postgres:postgres@localhost:5432/attune)
    --pack-dir DIR       Base directory for packs (default: ./packs)
    --python PATH        Path to Python interpreter (default: python3)
    --dry-run            Show what would be done without making changes
    -v, --verbose        Show detailed output
    -h, --help          Show this help message

Environment Variables:
    DATABASE_URL         PostgreSQL connection string
    ATTUNE_PACKS_DIR    Base directory for packs

Examples:
    # Load core pack with default settings
    $0

    # Use custom database URL
    $0 --database-url "postgresql://user:pass@db:5432/attune"

    # Dry run to see what would happen
    $0 --dry-run

EOF
            exit 0
            ;;
        *)
            error "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Print banner
echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Attune Core Pack Loader"
echo "═══════════════════════════════════════════════════════════"
echo ""

# Check prerequisites
info "Checking prerequisites..."

# Check Python
if ! command -v "$PYTHON_BIN" &> /dev/null; then
    error "Python 3 is required but not found: $PYTHON_BIN"
    echo "  Install Python 3.8+ and try again"
    exit 1
fi
success "Python 3 found: $($PYTHON_BIN --version)"

# Check Python packages
MISSING_PACKAGES=()

if ! "$PYTHON_BIN" -c "import psycopg2" 2>/dev/null; then
    MISSING_PACKAGES+=("psycopg2-binary")
fi

if ! "$PYTHON_BIN" -c "import yaml" 2>/dev/null; then
    MISSING_PACKAGES+=("pyyaml")
fi

if [ ${#MISSING_PACKAGES[@]} -gt 0 ]; then
    warning "Missing required Python packages: ${MISSING_PACKAGES[*]}"
    echo ""
    echo "Install them with:"
    echo "  pip install ${MISSING_PACKAGES[*]}"
    echo ""
    read -p "Install now? (y/N): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        info "Installing packages..."
        pip install "${MISSING_PACKAGES[@]}"
        success "Packages installed"
    else
        error "Required packages not installed"
        exit 1
    fi
fi
success "Python packages installed"

# Check database connectivity
info "Testing database connection..."
if ! pg_isready -d "$DATABASE_URL" -q 2>/dev/null; then
    # Try psql as fallback
    if ! psql "$DATABASE_URL" -c "SELECT 1" >/dev/null 2>&1; then
        error "Cannot connect to database"
        echo "  DATABASE_URL endpoint: $(sanitize_database_url "$DATABASE_URL")"
        echo ""
        echo "Troubleshooting:"
        echo "  - Check PostgreSQL is running"
        echo "  - Verify DATABASE_URL is correct"
        echo "  - Ensure database exists"
        exit 1
    fi
fi
success "Database connection OK"

# Check packs directory
info "Checking packs directory..."
if [ ! -d "$PACKS_DIR/core" ]; then
    error "Core pack directory not found: $PACKS_DIR/core"
    exit 1
fi
success "Core pack directory found"

# Check pack.yaml exists
if [ ! -f "$PACKS_DIR/core/pack.yaml" ]; then
    error "pack.yaml not found in core pack directory"
    exit 1
fi
success "pack.yaml found"

echo ""
info "Configuration:"
echo "  Database URL: $(sanitize_database_url "$DATABASE_URL")"
echo "  Packs Directory: $PACKS_DIR"
echo "  Core Pack: $PACKS_DIR/core"
echo ""

# Run the Python loader
LOADER_SCRIPT="$SCRIPT_DIR/load_core_pack.py"

if [ ! -f "$LOADER_SCRIPT" ]; then
    error "Loader script not found: $LOADER_SCRIPT"
    exit 1
fi

LOADER_ARGS=(
    "--database-url" "$DATABASE_URL"
    "--pack-dir" "$PACKS_DIR"
)

if [ "$DRY_RUN" = true ]; then
    LOADER_ARGS+=("--dry-run")
fi

if [ "$VERBOSE" = true ]; then
    info "Running loader with verbose output..."
    "$PYTHON_BIN" "$LOADER_SCRIPT" "${LOADER_ARGS[@]}"
else
    "$PYTHON_BIN" "$LOADER_SCRIPT" "${LOADER_ARGS[@]}"
fi

LOADER_EXIT_CODE=$?

echo ""

if [ $LOADER_EXIT_CODE -eq 0 ]; then
    echo "═══════════════════════════════════════════════════════════"
    success "Core pack loaded successfully!"
    echo "═══════════════════════════════════════════════════════════"
    echo ""
    echo "Next steps:"
    echo "  1. Verify: attune pack show core"
    echo "  2. List actions: attune action list --pack core"
    echo "  3. Create a rule using core triggers and actions"
    echo ""
else
    echo "═══════════════════════════════════════════════════════════"
    error "Failed to load core pack"
    echo "═══════════════════════════════════════════════════════════"
    echo ""
    echo "Check the error messages above for details"
    exit $LOADER_EXIT_CODE
fi
