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
BINARY=""
FILTER=""
EXTRA_ARGS=()

# ── Parse arguments ──────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --crate|-c)
      CRATE="$2"; shift 2 ;;
    --test)
      BINARY="$2"; shift 2 ;;
    --filter|-f)
      FILTER="$2"; shift 2 ;;
    --)
      shift; EXTRA_ARGS=("$@"); break ;;
    -h|--help)
      echo "Usage: entrypoint.sh [options] [-- cargo-test-args]"
      echo ""
      echo "Options:"
      echo "  --crate, -c <name>   Run tests for a specific crate (common, api, executor, worker)"
      echo "  --test <name>        Run one integration-test executable"
      echo "  --filter, -f <expr>  Filter test names (passed to cargo test as filter)"
      echo "  -- <args>            Extra args passed to the test binary (e.g. --nocapture)"
      echo ""
      echo "Environment:"
      echo "  DATABASE_URL         PostgreSQL connection string (required)"
      echo "  TEST_THREADS         Number of parallel test threads (default: 1)"
      echo "  ATTUNE_RUST_INCLUDE_EXTERNAL=1  Include tests requiring API, MinIO, CLI, or stress resources"
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

sanitize_database_url() {
  local at_signs=${1//[^@]/}
  if [[ ${#at_signs} -gt 1 || $1 == *$'\n'* || $1 == *$'\r'* ]]; then
    printf '%s\n' '<database-url configured>'
    return
  fi
  printf '%s\n' "$1" | sed -E 's#(://).*@#\1#; s#[?#].*$##'
}

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
umask 077
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
  url: ${ATTUNE__MESSAGE_QUEUE__URL:-amqp://attune:attune@rabbitmq:5672/attune_${ATTUNE_E2E_RUN_ID:-rust_int}}
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

sensor:
  notifier_ws_url: ws://127.0.0.1:8081/ws

pack_registry:
  enabled: true
  default_registry: https://registry.attune.example.com
  cache_ttl: 300
  approved_public_hosts:
    - registry.attune.example.com
    - raw.githubusercontent.com
    - github.com
    - codeload.github.com
    - objects.githubusercontent.com
EOF
chmod 600 /build/config.test.yaml

# ── Select precompiled test executables ──────────────────────────────────
TEST_THREADS="${TEST_THREADS:-1}"
MANIFEST=/build/test-artifacts/manifest.tsv
PACKAGE=""

if [[ -n "$CRATE" ]]; then
  case "$CRATE" in
    common|api|executor|sensor|worker|notifier|supervisor|cli)
      PACKAGE="attune-${CRATE}" ;;
    *)
      echo -e "${RED}ERROR: unsupported crate '${CRATE}'${NC}" >&2
      exit 2 ;;
  esac
fi

TEST_ARGS=()
if [[ -n "$FILTER" ]]; then
  TEST_ARGS+=("$FILTER")
fi
TEST_ARGS+=(--ignored --test-threads="$TEST_THREADS")
DEFAULT_SKIPS=()
if [[ "${ATTUNE_RUST_INCLUDE_EXTERNAL:-0}" != "1" ]]; then
  DEFAULT_SKIPS=(
    test_sse_stream_receives_execution_updates
    test_sse_stream_filters_by_execution_id
    test_sse_stream_requires_authentication
    test_sse_stream_all_executions
    dashboard_timezone_bucketing_handles_dst_and_non_hour_offsets
    test_action_execute_with_profile
    test_high_concurrency_stress
    test_extreme_stress_10k_executions
    s3_direct_upload_authorization_puts_and_verifies_exact_bytes
    log_segment_upload_goes_from_manager_to_minio_without_api_body_relay
    object_minio_duplicate_ambiguous_and_finalize_orderings
    object_minio_reader_recovers_missed_notifications_and_terminal
    object_minio_upload_reconnect_and_pinned_reads
    ordinary_artifact_upload_goes_from_manager_to_minio_without_api_body_relay
    shared_volume_cross_process_locking_writer_loss_and_retention
  )
  for skipped_test in "${DEFAULT_SKIPS[@]}"; do
    TEST_ARGS+=(--skip "$skipped_test")
  done
fi
if [[ ${#EXTRA_ARGS[@]} -gt 0 ]]; then
  TEST_ARGS+=("${EXTRA_ARGS[@]}")
fi

if [[ ! -s "$MANIFEST" ]]; then
  echo -e "${RED}ERROR: precompiled test artifact manifest is missing${NC}" >&2
  exit 1
fi
mapfile -t TEST_ENTRIES < <(
  if [[ -n "$PACKAGE" ]]; then
    awk -F '\t' -v package="$PACKAGE" '$1 == package' "$MANIFEST"
  else
    cat "$MANIFEST"
  fi
)
if [[ ${#TEST_ENTRIES[@]} -eq 0 ]]; then
  echo -e "${RED}ERROR: no precompiled test executables found for '${PACKAGE:-workspace}'${NC}" >&2
  exit 1
fi

if [[ -n "$BINARY" ]]; then
  MATCHING_ENTRIES=()
  for test_entry in "${TEST_ENTRIES[@]}"; do
    IFS=$'\t' read -r _ test_binary <<< "$test_entry"
    test_name="${test_binary##*/}"
    if [[ "$test_name" == "$BINARY"-* ]]; then
      MATCHING_ENTRIES+=("$test_entry")
    fi
  done
  TEST_ENTRIES=("${MATCHING_ENTRIES[@]}")
  if [[ ${#TEST_ENTRIES[@]} -eq 0 ]]; then
    echo -e "${RED}ERROR: no precompiled test executable matched '${BINARY}' in '${PACKAGE:-workspace}'${NC}" >&2
    exit 1
  fi
fi

if [[ -n "$FILTER" ]]; then
  MATCHING_ENTRIES=()
  for test_entry in "${TEST_ENTRIES[@]}"; do
    IFS=$'\t' read -r _ test_binary <<< "$test_entry"
    if "$test_binary" "$FILTER" --list --ignored | grep -E ': (test|benchmark)$' >/dev/null; then
      MATCHING_ENTRIES+=("$test_entry")
    fi
  done
  TEST_ENTRIES=("${MATCHING_ENTRIES[@]}")
  if [[ ${#TEST_ENTRIES[@]} -eq 0 ]]; then
    echo -e "${RED}ERROR: no ignored tests matched filter '${FILTER}' in '${PACKAGE:-workspace}'${NC}" >&2
    exit 1
  fi
fi

INVENTORY_SHA256="$(sha256sum /build/test-artifacts/inventory.tsv | cut -d ' ' -f 1)"
INVENTORY_TESTS="$(wc -l < /build/test-artifacts/inventory.tsv)"
SELECTED_INVENTORY="$(mktemp)"
for test_entry in "${TEST_ENTRIES[@]}"; do
  IFS=$'\t' read -r test_package test_binary <<< "$test_entry"
  test_name="${test_binary##*/}"
  if [[ "$test_name" =~ ^(.+)-[0-9a-f]{16}$ ]]; then
    test_target="${BASH_REMATCH[1]}"
  else
    test_target="$test_name"
  fi
  if ! listed_tests="$("$test_binary" "${TEST_ARGS[@]}" --list)"; then
    echo -e "${RED}ERROR: could not list selected tests in '${test_binary##*/}'${NC}" >&2
    rm -f "$SELECTED_INVENTORY"
    exit 1
  fi
  while IFS= read -r listed_test; do
    if [[ "$listed_test" =~ ^(.+):\ (test|benchmark)$ ]]; then
      printf '%s\t%s\t%s\n' \
        "$test_package" "$test_target" "${BASH_REMATCH[1]}" >> "$SELECTED_INVENTORY"
    fi
  done <<< "$listed_tests"
done
LC_ALL=C sort -o "$SELECTED_INVENTORY" "$SELECTED_INVENTORY"
SELECTED_TESTS="$(wc -l < "$SELECTED_INVENTORY")"
SELECTED_SHA256="$(sha256sum "$SELECTED_INVENTORY" | cut -d ' ' -f 1)"
rm -f "$SELECTED_INVENTORY"

# ── Print banner ─────────────────────────────────────────────────────────
echo ""
echo -e "${CYAN}╔════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  Attune Rust Integration Tests                        ║${NC}"
echo -e "${CYAN}╚════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "  ${YELLOW}DB:${NC}     $(sanitize_database_url "$DATABASE_URL")"
echo -e "  ${YELLOW}Crate:${NC}  ${CRATE:-all}"
echo -e "  ${YELLOW}Test:${NC}   ${BINARY:-all}"
echo -e "  ${YELLOW}Filter:${NC} ${FILTER:-<none>}"
echo -e "  ${YELLOW}Threads:${NC} $TEST_THREADS"
echo -e "  ${YELLOW}Bins:${NC}   ${#TEST_ENTRIES[@]} precompiled executables"
echo -e "  ${YELLOW}Tests:${NC}  $SELECTED_TESTS selected / $INVENTORY_TESTS inventoried"
echo -e "  ${YELLOW}Inventory:${NC} $INVENTORY_SHA256"
echo -e "  ${YELLOW}Selection:${NC} $SELECTED_SHA256"
printf 'ATTUNE_TEST_SELECTION selected=%s selected_sha256=%s inventoried=%s inventory_sha256=%s\n' \
  "$SELECTED_TESTS" "$SELECTED_SHA256" "$INVENTORY_TESTS" "$INVENTORY_SHA256"
echo -e "  ${YELLOW}External:${NC} ${ATTUNE_RUST_INCLUDE_EXTERNAL:-0} (${#DEFAULT_SKIPS[@]} default exclusions)"
echo -e "  ${YELLOW}Args:${NC}   ${TEST_ARGS[*]}"
echo ""

# ── Run tests ────────────────────────────────────────────────────────────
cd /build
EXIT_CODE=0
CURRENT_TEST_DATABASE=""

cleanup_current_test_database() {
  if [[ -n "$CURRENT_TEST_DATABASE" ]]; then
    /build/test-artifacts/test_database_lifecycle \
      drop /build/config.test.yaml "$CURRENT_TEST_DATABASE" || true
  fi
}
trap cleanup_current_test_database EXIT
trap 'exit 130' HUP INT TERM

for test_entry in "${TEST_ENTRIES[@]}"; do
  IFS=$'\t' read -r test_package test_binary <<< "$test_entry"
  echo -e "\n${CYAN}Running ${test_binary#/build/test-artifacts/}${NC}"
  test_database_url=""
  if [[ "$(basename "$test_binary")" == action_repository_tests-* ]]; then
    detached_database="$(/build/test-artifacts/test_database_lifecycle create /build/config.test.yaml)"
    IFS=$'\t' read -r CURRENT_TEST_DATABASE test_database_url <<< "$detached_database"
  fi

  test_status=0
  if [[ -n "$test_database_url" ]]; then
    CARGO_MANIFEST_DIR="/build/crates/${test_package#attune-}" \
      DATABASE_URL="$test_database_url" \
      ATTUNE__DATABASE__URL="$test_database_url" \
      ATTUNE_TEST_DATABASE_URL="$test_database_url" \
      ATTUNE_TEST_EXEC_DATABASE_NAME="$CURRENT_TEST_DATABASE" \
      "$test_binary" "${TEST_ARGS[@]}" || test_status=$?
  else
    CARGO_MANIFEST_DIR="/build/crates/${test_package#attune-}" \
      "$test_binary" "${TEST_ARGS[@]}" || test_status=$?
  fi

  if [[ -n "$CURRENT_TEST_DATABASE" ]]; then
    if ! /build/test-artifacts/test_database_lifecycle \
        drop /build/config.test.yaml "$CURRENT_TEST_DATABASE"; then
      test_status=1
    fi
    CURRENT_TEST_DATABASE=""
  fi
  if ((test_status != 0)); then
    EXIT_CODE=1
  fi
done
trap - EXIT HUP INT TERM
exit "$EXIT_CODE"
