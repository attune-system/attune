#!/bin/bash
# Migration script for Attune database
# Runs all SQL migration files in order

set -e

echo "=========================================="
echo "Attune Database Migration Runner"
echo "=========================================="
echo ""

# Database connection parameters
DB_HOST="${DB_HOST:-postgres}"
DB_PORT="${DB_PORT:-5432}"
DB_USER="${DB_USER:-attune}"
DB_PASSWORD="${DB_PASSWORD:-attune}"
DB_NAME="${DB_NAME:-attune}"

MIGRATIONS_DIR="${MIGRATIONS_DIR:-/migrations}"

# Use a private per-runner file. mktemp creates it mode 0600 and a process-local
# path prevents concurrent or privileged runners from clobbering one another.
MIGRATION_OUTPUT=$(mktemp "${TMPDIR:-/tmp}/attune-migration.XXXXXX")
cleanup() {
    rm -f -- "$MIGRATION_OUTPUT"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

# Export password for psql
export PGPASSWORD="$DB_PASSWORD"

# Set search_path for all psql connections so DDL lands in the attune schema
export PGOPTIONS="-c search_path=attune,public"

# Color output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to wait for PostgreSQL to be ready
wait_for_postgres() {
    echo "Waiting for PostgreSQL to be ready..."
    local max_attempts=30
    local attempt=1

    while [ $attempt -le $max_attempts ]; do
        if psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -c '\q' 2>/dev/null; then
            echo -e "${GREEN}✓ PostgreSQL is ready${NC}"
            return 0
        fi

        echo "  Attempt $attempt/$max_attempts: PostgreSQL not ready yet..."
        sleep 2
        attempt=$((attempt + 1))
    done

    echo -e "${RED}✗ PostgreSQL failed to become ready after $max_attempts attempts${NC}"
    return 1
}

# Function to check if migrations table exists
setup_migrations_table() {
    echo "Setting up migrations tracking table..."

    psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -v ON_ERROR_STOP=1 <<-'EOSQL' || return 1
        BEGIN;
        SELECT pg_advisory_xact_lock(78210015);

        DO $$
        DECLARE
            has_attune_sqlx BOOLEAN := to_regclass('attune._sqlx_migrations') IS NOT NULL;
            has_public_sqlx BOOLEAN := to_regclass('public._sqlx_migrations') IS NOT NULL;
            has_attune_docker BOOLEAN := to_regclass('attune._migrations') IS NOT NULL;
            has_public_docker BOOLEAN := to_regclass('public._migrations') IS NOT NULL;
            sqlx_history REGCLASS;
            sqlx_migration_count BIGINT;
        BEGIN
            IF has_attune_sqlx AND has_public_sqlx THEN
                RAISE EXCEPTION 'Database contains SQLx migration history in both attune and public schemas';
            END IF;
            IF has_attune_docker AND has_public_docker THEN
                RAISE EXCEPTION 'Database contains Docker migration history in both attune and public schemas';
            END IF;
            IF (has_attune_sqlx OR has_public_sqlx)
                AND (has_attune_docker OR has_public_docker) THEN
                RAISE EXCEPTION 'Database contains mixed SQLx and Docker migration history';
            END IF;

            IF has_attune_sqlx OR has_public_sqlx THEN
                sqlx_history := CASE
                    WHEN has_attune_sqlx THEN 'attune._sqlx_migrations'::regclass
                    ELSE 'public._sqlx_migrations'::regclass
                END;
                EXECUTE format('SELECT COUNT(*) FROM %s', sqlx_history)
                INTO sqlx_migration_count;

                IF sqlx_migration_count > 0 THEN
                    RAISE EXCEPTION 'Database uses SQLx migration history; run attune-api --migrate instead of the Docker migration container';
                END IF;

                -- SQLx creates this table before source migrations. A rejected
                -- takeover therefore leaves an empty table that is safe to
                -- remove while the shared advisory lock excludes both runners.
                EXECUTE format('DROP TABLE %s', sqlx_history);
            END IF;

            IF has_public_docker THEN
                CREATE SCHEMA IF NOT EXISTS attune;
                ALTER TABLE public._migrations SET SCHEMA attune;
            END IF;
        END
        $$;

        CREATE SCHEMA IF NOT EXISTS attune;
        CREATE TABLE IF NOT EXISTS public._attune_migration_runner (
            id SMALLINT PRIMARY KEY CHECK (id = 1),
            runner TEXT NOT NULL CHECK (runner IN ('sqlx', 'docker')),
            claimed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            legacy_checksum_adoption BOOLEAN NOT NULL DEFAULT FALSE
        );
        ALTER TABLE public._attune_migration_runner
            ADD COLUMN IF NOT EXISTS legacy_checksum_adoption BOOLEAN NOT NULL DEFAULT FALSE;

        INSERT INTO public._attune_migration_runner (id, runner)
        VALUES (1, 'docker')
        ON CONFLICT (id) DO NOTHING;

        DO $$
        BEGIN
            IF (SELECT runner FROM public._attune_migration_runner WHERE id = 1) <> 'docker' THEN
                RAISE EXCEPTION 'Database is claimed by the SQLx migration runner; run attune-api --migrate instead of the Docker migration container';
            END IF;
        END
        $$;

        DO $$
        DECLARE
            history_existed BOOLEAN := to_regclass('attune._migrations') IS NOT NULL;
            history_had_checksum BOOLEAN := FALSE;
            history_count BIGINT := 0;
        BEGIN
            IF history_existed THEN
                SELECT EXISTS (
                    SELECT 1
                    FROM pg_attribute
                    WHERE attrelid = 'attune._migrations'::regclass
                      AND attname = 'checksum_sha384'
                      AND NOT attisdropped
                )
                INTO history_had_checksum;

                SELECT COUNT(*) INTO history_count FROM attune._migrations;
            END IF;

            CREATE TABLE IF NOT EXISTS attune._migrations (
                id SERIAL PRIMARY KEY,
                filename VARCHAR(255) UNIQUE NOT NULL,
                checksum_sha384 VARCHAR(96) NOT NULL,
                applied_at TIMESTAMP DEFAULT NOW()
            );
            ALTER TABLE attune._migrations
                ADD COLUMN IF NOT EXISTS checksum_sha384 VARCHAR(96);

            IF NOT EXISTS (
                SELECT 1
                FROM pg_constraint
                WHERE conrelid = 'attune._migrations'::regclass
                  AND conname = '_migrations_checksum_sha384_format'
            ) THEN
                ALTER TABLE attune._migrations
                    ADD CONSTRAINT _migrations_checksum_sha384_format
                    CHECK (
                        checksum_sha384 IS NULL
                        OR checksum_sha384 ~ '^[0-9a-f]{96}$'
                    );
            END IF;

            -- A pre-checksum history cannot prove what bytes originally ran.
            -- Permit one complete runner pass to baseline files still shipped,
            -- then close adoption so later NULLs fail rather than being trusted.
            IF history_existed AND history_count > 0 AND NOT history_had_checksum THEN
                UPDATE public._attune_migration_runner
                SET legacy_checksum_adoption = TRUE
                WHERE id = 1 AND runner = 'docker';
            ELSIF history_existed AND history_count = 0 AND NOT history_had_checksum THEN
                ALTER TABLE attune._migrations
                    ALTER COLUMN checksum_sha384 SET NOT NULL;
            END IF;
        END
        $$;
        COMMIT;
EOSQL

    psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -v ON_ERROR_STOP=1 <<-EOSQL
        -- Set default search_path for the database so all connections use attune schema
        ALTER DATABASE "$DB_NAME" SET search_path TO attune, public;

        -- Set search_path for this session
        SET search_path TO attune, public;
EOSQL

    echo -e "${GREEN}✓ Migrations table ready${NC}"
}

# Function to run a migration file
run_migration() {
    local filepath=$1
    local filename=$(basename "$filepath")
    echo -e "${GREEN}→ Applying $filename...${NC}"

    # Serialize same-runner migrations and commit DDL with its history record.
    if psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" \
        -v ON_ERROR_STOP=1 \
        -v migration_filename="$filename" \
        -v migration_filepath="$filepath" \
        > "$MIGRATION_OUTPUT" 2>&1 <<-'EOSQL'
            BEGIN;
            SELECT pg_advisory_xact_lock(78210015);

            -- Import the client-side file as a transactional large object so
            -- PostgreSQL's built-in SHA-384 implementation hashes exact bytes.
            \lo_import :migration_filepath
            SELECT encode(sha384(lo_get(:LASTOID)), 'hex') AS migration_checksum \gset
            SELECT lo_unlink(:LASTOID);

            SELECT
                EXISTS (
                    SELECT 1 FROM attune._migrations
                    WHERE filename = :'migration_filename'
                ) AS already_applied,
                COALESCE((
                    SELECT checksum_sha384 = :'migration_checksum'
                    FROM attune._migrations
                    WHERE filename = :'migration_filename'
                ), FALSE) AS checksum_matches,
                EXISTS (
                    SELECT 1 FROM attune._migrations
                    WHERE filename = :'migration_filename'
                      AND checksum_sha384 IS NULL
                ) AS checksum_missing,
                (SELECT legacy_checksum_adoption
                 FROM public._attune_migration_runner
                 WHERE id = 1) AS legacy_checksum_adoption
            \gset
            \if :already_applied
                \if :checksum_matches
                    ROLLBACK;
                    \echo ATTUNE_MIGRATION_SKIPPED
                \elif :checksum_missing
                    \if :legacy_checksum_adoption
                        UPDATE attune._migrations
                        SET checksum_sha384 = :'migration_checksum'
                        WHERE filename = :'migration_filename'
                          AND checksum_sha384 IS NULL;
                        COMMIT;
                        \echo ATTUNE_MIGRATION_CHECKSUM_ADOPTED
                        \echo ATTUNE_MIGRATION_SKIPPED
                    \else
                        DO $$
                        BEGIN
                            RAISE EXCEPTION 'Migration history has no SHA-384 checksum outside the legacy adoption pass';
                        END
                        $$;
                    \endif
                \else
                    DO $$
                    BEGIN
                        RAISE EXCEPTION 'Migration SHA-384 checksum does not match the stored history';
                    END
                    $$;
                \endif
            \else
                \i :migration_filepath
                INSERT INTO attune._migrations (filename, checksum_sha384)
                VALUES (:'migration_filename', :'migration_checksum');
                COMMIT;
                \echo ATTUNE_MIGRATION_APPLIED
            \endif
EOSQL
    then
        if grep -q 'ATTUNE_MIGRATION_SKIPPED' "$MIGRATION_OUTPUT"; then
            echo -e "${YELLOW}⊘ Skipping $filename (already applied)${NC}"
            return 2
        fi
        echo -e "${GREEN}✓ Applied $filename${NC}"
        return 0
    else
        echo -e "${RED}✗ Failed to apply $filename${NC}"
        echo ""
        echo "Error details:"
        cat "$MIGRATION_OUTPUT"
        echo ""
        echo "Migration rolled back due to error."
        return 1
    fi
}

finalize_legacy_checksum_adoption() {
    psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" \
        -v ON_ERROR_STOP=1 <<-'EOSQL'
            BEGIN;
            SELECT pg_advisory_xact_lock(78210015);
            UPDATE public._attune_migration_runner
            SET legacy_checksum_adoption = FALSE
            WHERE id = 1
              AND runner = 'docker'
              AND legacy_checksum_adoption;
            COMMIT;
EOSQL
}

# Function to initialize Docker-specific roles and extensions
init_docker_roles() {
    echo "Initializing Docker roles and extensions..."

    if [ -f "/docker/init-roles.sql" ]; then
        if psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -v ON_ERROR_STOP=1 -f "/docker/init-roles.sql" > /dev/null 2>&1; then
            echo -e "${GREEN}✓ Docker roles initialized${NC}"
            return 0
        else
            echo -e "${YELLOW}⚠ Warning: Could not initialize Docker roles (may already exist)${NC}"
            return 0
        fi
    else
        echo -e "${YELLOW}⚠ No Docker init script found, skipping${NC}"
        return 0
    fi
}

# Main migration process
main() {
    echo "Configuration:"
    echo "  Database: $DB_HOST:$DB_PORT/$DB_NAME"
    echo "  User: $DB_USER"
    echo "  Migrations directory: $MIGRATIONS_DIR"
    echo ""

    # Wait for database
    wait_for_postgres || exit 1

    # Initialize Docker-specific roles
    init_docker_roles || exit 1

    # Setup migrations tracking
    setup_migrations_table || exit 1

    echo ""
    echo "Running migrations..."
    echo "----------------------------------------"

    # Find and sort migration files
    local migration_count=0
    local applied_count=0
    local skipped_count=0

    # Process migrations in sorted order
    for migration_file in $(find "$MIGRATIONS_DIR" -name "*.sql" -type f | sort); do
        migration_count=$((migration_count + 1))

        if run_migration "$migration_file"; then
            applied_count=$((applied_count + 1))
        else
            migration_status=$?
            if [ "$migration_status" -eq 2 ]; then
                skipped_count=$((skipped_count + 1))
            else
                echo -e "${RED}Migration failed!${NC}"
                exit 1
            fi
        fi
    done

    if [ "$migration_count" -eq 0 ]; then
        echo -e "${RED}No migration files found; refusing to close checksum adoption${NC}"
        exit 1
    fi

    # Close the one-pass compatibility window only after every shipped file
    # was applied, verified, or baselined successfully.
    finalize_legacy_checksum_adoption || exit 1

    echo "----------------------------------------"
    echo ""
    echo "Migration Summary:"
    echo "  Total migrations: $migration_count"
    echo "  Newly applied: $applied_count"
    echo "  Already applied: $skipped_count"
    echo ""

    if [ $applied_count -gt 0 ]; then
        echo -e "${GREEN}✓ All migrations applied successfully!${NC}"
    else
        echo -e "${GREEN}✓ Database is up to date (no new migrations)${NC}"
    fi
}

# Run main function
main
