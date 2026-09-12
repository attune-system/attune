#!/bin/sh
# Initialize builtin packs for Attune
# This script copies pack files to the shared volume and registers them in the database
# Designed to run on python:3.11-slim (Debian-based) image

set -e

if [ "${ATTUNE_BOOTSTRAP_MODE:-volume}" = api ]; then
    exec python3 /scripts/bootstrap_core_pack.py publish
fi

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration from environment
DB_HOST="${DB_HOST:-postgres}"
DB_PORT="${DB_PORT:-5432}"
DB_USER="${DB_USER:-attune}"
DB_PASSWORD="${DB_PASSWORD:-attune}"
DB_NAME="${DB_NAME:-attune}"
DB_SCHEMA="${DB_SCHEMA:-attune}"

# Pack directories
SOURCE_PACKS_DIR="${SOURCE_PACKS_DIR:-/source/packs}"
TARGET_PACKS_DIR="${TARGET_PACKS_DIR:-/opt/attune/packs}"

# Python loader script
LOADER_SCRIPT="${LOADER_SCRIPT:-/scripts/load_core_pack.py}"
DEFAULT_ADMIN_LOGIN="${DEFAULT_ADMIN_LOGIN:-}"
DEFAULT_ADMIN_PERMISSION_SET_REF="${DEFAULT_ADMIN_PERMISSION_SET_REF:-core.admin}"

echo ""
echo -e "${BLUE}╔════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║    Attune Builtin Packs Initialization         ║${NC}"
echo -e "${BLUE}╚════════════════════════════════════════════════╝${NC}"
echo ""

# Install Python dependencies
echo -e "${YELLOW}→${NC} Installing Python dependencies..."
if pip install --quiet --no-cache-dir psycopg2-binary pyyaml; then
    echo -e "${GREEN}✓${NC} Python dependencies installed"
else
    echo -e "${RED}✗${NC} Failed to install Python dependencies"
    exit 1
fi
echo ""

# Wait for database to be ready (using Python instead of psql to avoid needing postgresql-client)
echo -e "${YELLOW}→${NC} Waiting for database to be ready..."
until python3 -c "
import psycopg2, sys
try:
    conn = psycopg2.connect(host='$DB_HOST', port=$DB_PORT, user='$DB_USER', password='$DB_PASSWORD', dbname='$DB_NAME', connect_timeout=3)
    conn.close()
    sys.exit(0)
except Exception:
    sys.exit(1)
" 2>/dev/null; do
  echo -e "${YELLOW}  ...${NC} Database is unavailable - sleeping"
  sleep 2
done
echo -e "${GREEN}✓${NC} Database is ready"

# Create target packs directory if it doesn't exist
echo -e "${YELLOW}→${NC} Ensuring packs directory exists..."
mkdir -p "$TARGET_PACKS_DIR"
# Ensure the attune user (uid 1000) can write to the packs directory
# so the API service can install packs at runtime
chown -R 1000:1000 "$TARGET_PACKS_DIR"
echo -e "${GREEN}✓${NC} Packs directory ready at: $TARGET_PACKS_DIR"

# Initialise runtime environments volume with correct ownership.
# Workers (running as attune uid 1000) need write access to create
# virtualenvs, node_modules, etc. at runtime.
RUNTIME_ENVS_DIR="${RUNTIME_ENVS_DIR:-/opt/attune/runtime_envs}"
if [ -d "$RUNTIME_ENVS_DIR" ] || mkdir -p "$RUNTIME_ENVS_DIR" 2>/dev/null; then
    chown -R 1000:1000 "$RUNTIME_ENVS_DIR"
    echo -e "${GREEN}✓${NC} Runtime environments directory ready at: $RUNTIME_ENVS_DIR"
else
    echo -e "${YELLOW}⚠${NC} Runtime environments directory not mounted, skipping"
fi

# Initialise artifacts volume with correct ownership.
# The API service (creates directories for file-backed artifact versions) and
# workers (write artifact files during execution) both run as attune uid 1000.
ARTIFACTS_DIR="${ARTIFACTS_DIR:-/opt/attune/artifacts}"
if [ -d "$ARTIFACTS_DIR" ] || mkdir -p "$ARTIFACTS_DIR" 2>/dev/null; then
    chown -R 1000:1000 "$ARTIFACTS_DIR"
    echo -e "${GREEN}✓${NC} Artifacts directory ready at: $ARTIFACTS_DIR"
else
    echo -e "${YELLOW}⚠${NC} Artifacts directory not mounted, skipping"
fi

# Check if source packs directory exists
if [ ! -d "$SOURCE_PACKS_DIR" ]; then
    echo -e "${RED}✗${NC} Source packs directory not found: $SOURCE_PACKS_DIR"
    exit 1
fi

# Find all pack directories (directories with pack.yaml)
echo ""
echo -e "${BLUE}Discovering builtin packs...${NC}"
echo "----------------------------------------"

PACK_COUNT=0
COPIED_COUNT=0
LOADED_COUNT=0

for pack_dir in "$SOURCE_PACKS_DIR"/*; do
    if [ -d "$pack_dir" ]; then
        pack_name=$(basename "$pack_dir")
        pack_yaml="$pack_dir/pack.yaml"

        if [ -f "$pack_yaml" ]; then
            PACK_COUNT=$((PACK_COUNT + 1))
            echo -e "${BLUE}→${NC} Found pack: ${GREEN}$pack_name${NC}"

            # Check if pack already exists in target
            target_pack_dir="$TARGET_PACKS_DIR/$pack_name"

            if [ -d "$target_pack_dir" ]; then
                # Pack exists, update non-binary files to ensure we have latest.
                # Compiled sensor binaries in sensors/ are provided by init-pack-binaries
                # (statically-linked musl builds) and must NOT be overwritten by
                # the host's copy, which may be dynamically linked or the wrong arch.
                echo -e "${YELLOW}  ⟳${NC} Pack exists at: $target_pack_dir, updating files..."

                # Detect ELF binaries already in the target sensors/ and
                # actions/ dirs by checking for the 4-byte ELF magic number
                # (\x7fELF) at the start of the file.  The `file` command is
                # unavailable on python:3.11-slim, so we read the magic
                # bytes with `od`. We track each preserved binary by its
                # path relative to the pack directory so the restore step
                # writes it back to the correct subdirectory.
                _skip_paths=""
                for _bin_dir in sensors actions ; do
                    if [ -d "$target_pack_dir/$_bin_dir" ]; then
                        for _bin in "$target_pack_dir/$_bin_dir"/* ; do
                            [ -f "$_bin" ] || continue
                            _magic=$(od -A n -t x1 -N 4 "$_bin" 2>/dev/null | tr -d ' ')
                            if [ "$_magic" = "7f454c46" ]; then
                                _skip_paths="$_skip_paths ${_bin_dir}/$(basename "$_bin")"
                            fi
                        done
                    fi
                done

                # Copy everything from source, then restore any skipped binaries
                # that were overwritten. We do it this way (copy-then-restore)
                # rather than exclude-during-copy because busybox cp and POSIX cp
                # have no --exclude flag.
                if [ -n "$_skip_paths" ]; then
                    # Back up existing static binaries
                    _tmpdir=$(mktemp -d)
                    for _p in $_skip_paths; do
                        mkdir -p "$_tmpdir/$(dirname "$_p")"
                        cp "$target_pack_dir/$_p" "$_tmpdir/$_p"
                    done
                fi

                if cp -rf "$pack_dir"/* "$target_pack_dir"/; then
                    echo -e "${GREEN}  ✓${NC} Updated pack files at: $target_pack_dir"
                else
                    echo -e "${RED}  ✗${NC} Failed to update pack"
                    exit 1
                fi

                # Restore static binaries that were overwritten
                if [ -n "$_skip_paths" ]; then
                    for _p in $_skip_paths; do
                        cp "$_tmpdir/$_p" "$target_pack_dir/$_p"
                        chmod +x "$target_pack_dir/$_p"
                        echo -e "${GREEN}  ✓${NC} Preserved static binary: $_p"
                    done
                    rm -rf "$_tmpdir"
                fi
            else
                # Copy pack to target directory
                echo -e "${YELLOW}  →${NC} Copying pack files..."
                if cp -r "$pack_dir" "$target_pack_dir"; then
                    COPIED_COUNT=$((COPIED_COUNT + 1))
                    echo -e "${GREEN}  ✓${NC} Copied to: $target_pack_dir"
                else
                    echo -e "${RED}  ✗${NC} Failed to copy pack"
                    exit 1
                fi
            fi
        fi
    fi
done

echo "----------------------------------------"
echo ""

if [ $PACK_COUNT -eq 0 ]; then
    echo -e "${YELLOW}⚠${NC} No builtin packs found in $SOURCE_PACKS_DIR"
    echo -e "${BLUE}ℹ${NC} This is OK if you're running with no packs"
    exit 0
fi

echo -e "${BLUE}Pack Discovery Summary:${NC}"
echo "  Total packs found: $PACK_COUNT"
echo "  Newly copied: $COPIED_COUNT"
echo "  Already present: $((PACK_COUNT - COPIED_COUNT))"
echo ""

# Load packs into database using Python loader
if [ -f "$LOADER_SCRIPT" ]; then
    echo -e "${BLUE}Loading packs into database...${NC}"
    echo "----------------------------------------"

    # Build database URL with schema support
    DATABASE_URL="postgresql://$DB_USER:$DB_PASSWORD@$DB_HOST:$DB_PORT/$DB_NAME"

    # Always set search_path so objects are created in the correct schema
    export PGOPTIONS="-c search_path=$DB_SCHEMA,public"

    # Run the Python loader for each pack
    for pack_dir in "$TARGET_PACKS_DIR"/*; do
        if [ -d "$pack_dir" ]; then
            pack_name=$(basename "$pack_dir")
            pack_yaml="$pack_dir/pack.yaml"

            if [ -f "$pack_yaml" ]; then
                echo -e "${YELLOW}→${NC} Loading pack: ${GREEN}$pack_name${NC}"

                # Run Python loader
                if python3 "$LOADER_SCRIPT" \
                    --database-url "$DATABASE_URL" \
                    --pack-dir "$TARGET_PACKS_DIR" \
                    --pack-name "$pack_name" \
                    --schema "$DB_SCHEMA" \
                    --storage-path "$pack_dir"; then
                    LOADED_COUNT=$((LOADED_COUNT + 1))
                    echo -e "${GREEN}✓${NC} Loaded pack: $pack_name"
                else
                    echo -e "${RED}✗${NC} Failed to load pack: $pack_name"
                    echo -e "${YELLOW}⚠${NC} Continuing with other packs..."
                fi
            fi
        fi
    done

    echo "----------------------------------------"
    echo ""
    echo -e "${BLUE}Database Loading Summary:${NC}"
    echo "  Successfully loaded: $LOADED_COUNT"
    echo "  Failed: $((PACK_COUNT - LOADED_COUNT))"
    echo ""
else
    echo -e "${YELLOW}⚠${NC} Pack loader script not found: $LOADER_SCRIPT"
    echo -e "${BLUE}ℹ${NC} Packs copied but not registered in database"
    echo -e "${BLUE}ℹ${NC} You can manually load them later"
fi

if [ -n "$DEFAULT_ADMIN_LOGIN" ] && [ "$LOADED_COUNT" -gt 0 ]; then
    echo ""
    echo -e "${BLUE}Bootstrapping local admin assignment...${NC}"
    if python3 - <<PY
import psycopg2
import sys

conn = psycopg2.connect(
    host="${DB_HOST}",
    port=${DB_PORT},
    user="${DB_USER}",
    password="${DB_PASSWORD}",
    dbname="${DB_NAME}",
)
conn.autocommit = False

try:
    with conn.cursor() as cur:
        cur.execute("SET search_path TO ${DB_SCHEMA}, public")
        cur.execute("SELECT id FROM identity WHERE login = %s", ("${DEFAULT_ADMIN_LOGIN}",))
        identity_row = cur.fetchone()
        if identity_row is None:
            print("  ⚠ Default admin identity not found; skipping assignment")
            conn.rollback()
            sys.exit(0)

        cur.execute("SELECT id FROM permission_set WHERE ref = %s", ("${DEFAULT_ADMIN_PERMISSION_SET_REF}",))
        permset_row = cur.fetchone()
        if permset_row is None:
            print("  ⚠ Default admin permission set not found; skipping assignment")
            conn.rollback()
            sys.exit(0)

        cur.execute(
            """
            INSERT INTO permission_assignment (identity, permset)
            VALUES (%s, %s)
            ON CONFLICT (identity, permset) DO NOTHING
            """,
            (identity_row[0], permset_row[0]),
        )
    conn.commit()
    print("  ✓ Default admin permission assignment ensured")
except Exception as exc:
    conn.rollback()
    print(f"  ✗ Failed to ensure default admin assignment: {exc}")
    sys.exit(1)
finally:
    conn.close()
PY
    then
        :
    else
        exit 1
    fi
fi

# Summary
echo ""
echo -e "${GREEN}╔════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║    Builtin Packs Initialization Complete!      ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${BLUE}Packs Location:${NC} ${GREEN}$TARGET_PACKS_DIR${NC}"
echo -e "${BLUE}Packs Available:${NC}"

for pack_dir in "$TARGET_PACKS_DIR"/*; do
    if [ -d "$pack_dir" ]; then
        pack_name=$(basename "$pack_dir")
        pack_yaml="$pack_dir/pack.yaml"
        if [ -f "$pack_yaml" ]; then
            # Try to extract version from pack.yaml
            version=$(grep "^version:" "$pack_yaml" | head -1 | sed 's/version:[[:space:]]*//' | tr -d '"')
            echo -e "  • ${GREEN}$pack_name${NC} ${BLUE}($version)${NC}"
        fi
    fi
done

echo ""
# Ensure ownership is correct after all packs have been copied
# The API service (running as attune uid 1000) needs write access to install new packs
chown -R 1000:1000 "$TARGET_PACKS_DIR"

echo -e "${BLUE}ℹ${NC} Pack files are accessible to all services via shared volume"
echo ""

exit 0
