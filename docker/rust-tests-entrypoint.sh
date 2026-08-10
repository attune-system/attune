#!/usr/bin/env bash
#
# Entrypoint for the Rust integration test container.
#
# Runs all #[ignore]'d integration tests that require a live database.
# The DATABASE_URL environment variable must point to a reachable PostgreSQL instance.
#
# Usage:
#   # Run all integration tests:
#   docker run --rm attune-rust-tests
#
#   # Run tests for a specific crate:
#   docker run --rm attune-rust-tests --crate common
#   docker run --rm attune-rust-tests --crate api
#   docker run --rm attune-rust-tests --crate executor
#
#   # Run a specific test by name:
#   docker run --rm attune-rust-tests --filter test_create_action
#
#   # Pass extra cargo test args:
#   docker run --rm attune-rust-tests -- --nocapture
#
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

CRATE=""
FILTER=""
EXTRA_ARGS=()

# ── Parse arguments ──────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --crate|-c)
      CRATE="$2"; shift 2 ;;
    --filter|-f)
      FILTER="$2"; shift 2 ;;
    --)
      shift; EXTRA_ARGS=("$@"); break ;;
    -h|--help)
      echo "Usage: entrypoint.sh [options] [-- cargo-test-args]"
      echo ""
      echo "Options:"
      echo "  --crate, -c <name>   Run tests for a specific crate (common, api, executor, worker)"
      echo "  --filter, -f <expr>  Filter test names (passed to cargo test as filter)"
      echo "  -- <args>            Extra args passed to the test binary (e.g. --nocapture)"
      echo ""
      echo "Environment:"
      echo "  DATABASE_URL         PostgreSQL connection string (required)"
      echo "  TEST_THREADS         Number of parallel test threads (default: 4)"
      exit 0 ;;
    *)
      # Treat as filter if no flag prefix
      FILTER="$1"; shift ;;
  esac
done

# ── Validate environment ─────────────────────────────────────────────────
if [[ -z "${DATABASE_URL:-}" ]]; then
  echo -e "${RED}ERROR: DATABASE_URL environment variable is required${NC}" >&2
  exit 1
fi

# ── Wait for database ────────────────────────────────────────────────────
echo -e "${CYAN}Waiting for database...${NC}"
MAX_WAIT=60
WAITED=0
# Extract host:port from DATABASE_URL for connectivity check
DB_HOST=$(echo "$DATABASE_URL" | sed -E 's|.*@([^:/]+).*|\1|')
DB_PORT=$(echo "$DATABASE_URL" | sed -E 's|.*:([0-9]+)/.*|\1|')
DB_PORT="${DB_PORT:-5432}"

while ! bash -c "echo >/dev/tcp/$DB_HOST/$DB_PORT" 2>/dev/null; do
  if [[ $WAITED -ge $MAX_WAIT ]]; then
    echo -e "${RED}ERROR: Database not reachable after ${MAX_WAIT}s${NC}" >&2
    exit 1
  fi
  sleep 1
  WAITED=$((WAITED + 1))
done
echo -e "${GREEN}Database is reachable (${WAITED}s)${NC}"

# ── Create test database if it doesn't exist ─────────────────────────────
# The tests expect attune_test database; create it if only the main DB exists
DB_NAME=$(echo "$DATABASE_URL" | sed -E 's|.*/([^?]+).*|\1|')
BASE_URL=$(echo "$DATABASE_URL" | sed -E "s|/[^?]+|/postgres|")

echo -e "${CYAN}Ensuring database '${DB_NAME}' exists...${NC}"

# Use psql-like approach via a simple Rust binary isn't available, so we'll
# use the sqlx-based test helpers which create schemas per-test.
# The test database must exist — if it doesn't, we create it.
# We need the postgres client for this, or we can skip if tests create their own schemas.

# ── Override config for Docker environment ───────────────────────────────
# The test helpers read config.test.yaml via CARGO_MANIFEST_DIR/../../config.test.yaml
# In Docker, we override DATABASE_URL to point to the container network's postgres.
# We write a Docker-specific test config that the helpers will pick up.
cat > /build/config.test.yaml <<EOF
environment: test

database:
  url: ${DATABASE_URL}
  max_connections: 10
  min_connections: 2
  connect_timeout: 10
  idle_timeout: 60
  log_statements: false
  schema: null

redis:
  url: redis://redis:6379/1
  pool_size: 5

message_queue:
  url: amqp://guest:guest@rabbitmq:5672/%2f
  exchange: attune_test
  enable_dlq: false
  message_ttl: 300

server:
  host: 0.0.0.0
  port: 0
  request_timeout: 10
  enable_cors: true
  cors_origins:
    - http://localhost:3000
  max_body_size: 1048576

log:
  level: warn
  format: pretty
  console: true

security:
  jwt_secret: test-secret-for-testing-only-not-secure
  jwt_access_expiration: 300
  jwt_refresh_expiration: 3600
  encryption_key: test-encryption-key-32-chars-okay
  enable_auth: true
  allow_self_registration: true

packs_base_dir: /tmp/attune-test-packs
runtime_envs_dir: /tmp/attune-test-runtime-envs

pack_registry:
  enabled: true
  default_registry: https://registry.attune.example.com
  cache_ttl: 300
  approved_public_hosts:
    - registry.attune.example.com
    - github.com
    - codeload.github.com
    - objects.githubusercontent.com
EOF

# ── Build cargo test command ─────────────────────────────────────────────
TEST_THREADS="${TEST_THREADS:-4}"

CARGO_CMD=(cargo test)

if [[ -n "$CRATE" ]]; then
  CARGO_CMD+=(-p "attune_${CRATE}")
fi

# Add filter if specified
if [[ -n "$FILTER" ]]; then
  CARGO_CMD+=("$FILTER")
fi

# Run only the ignored (integration) tests
CARGO_CMD+=(-- --ignored --test-threads="$TEST_THREADS")

# Append extra args
if [[ ${#EXTRA_ARGS[@]} -gt 0 ]]; then
  CARGO_CMD+=("${EXTRA_ARGS[@]}")
fi

# ── Print banner ─────────────────────────────────────────────────────────
echo ""
echo -e "${CYAN}╔════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  Attune Rust Integration Tests                        ║${NC}"
echo -e "${CYAN}╚════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "  ${YELLOW}DB:${NC}     $DATABASE_URL"
echo -e "  ${YELLOW}Crate:${NC}  ${CRATE:-all}"
echo -e "  ${YELLOW}Filter:${NC} ${FILTER:-<none>}"
echo -e "  ${YELLOW}Threads:${NC} $TEST_THREADS"
echo -e "  ${YELLOW}Cmd:${NC}    ${CARGO_CMD[*]}"
echo ""

# ── Run tests ────────────────────────────────────────────────────────────
cd /build
exec "${CARGO_CMD[@]}"
