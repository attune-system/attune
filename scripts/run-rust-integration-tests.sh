#!/usr/bin/env bash
#
# Orchestration script for Rust integration tests in Docker.
#
# Starts the owned PostgreSQL and RabbitMQ dependencies, builds the Rust test
# container, runs the #[ignore]'d integration tests, and tears down.
#
# Usage:
#   ./scripts/run-rust-integration-tests.sh              # All crates
#   ./scripts/run-rust-integration-tests.sh --crate common  # Specific crate
#   ./scripts/run-rust-integration-tests.sh --crate api     # API tests
#   ./scripts/run-rust-integration-tests.sh --filter test_create_action
#   ./scripts/run-rust-integration-tests.sh --test action_repository_tests
#   ./scripts/run-rust-integration-tests.sh --no-teardown   # Keep DB running
#   ./scripts/run-rust-integration-tests.sh --no-build      # Skip rebuild
#
set -euo pipefail

# ── Colours ───────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# ── Defaults ──────────────────────────────────────────────────────────────
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILES=("-f" "$PROJECT_ROOT/docker-compose.yaml" "-f" "$PROJECT_ROOT/docker-compose.e2e.yaml")
validate_identifier() {
  [[ "$1" =~ ^[a-z0-9][a-z0-9_-]{0,47}$ ]]
}
RUN_ID="${ATTUNE_E2E_RUN_ID:-$(date -u +%Y%m%d%H%M%S)-$$-${RANDOM}}"
COMPOSE_PROJECT_NAME="${ATTUNE_E2E_PROJECT_NAME:-attune-rust-int-${RUN_ID}}"
export COMPOSE_PROJECT_NAME
if ! validate_identifier "$RUN_ID" || ! validate_identifier "$COMPOSE_PROJECT_NAME"; then
  echo "ERROR: test run/project identifiers must be lowercase safe identifiers of at most 48 characters" >&2
  exit 2
fi
export ATTUNE_E2E_RUN_ID="$RUN_ID"
ATTUNE_TEST_RUN_ID="${ATTUNE_TEST_RUN_ID:-r$(printf '%s' "$RUN_ID" | sha256sum | cut -c1-19)}"
if [[ ! "$ATTUNE_TEST_RUN_ID" =~ ^[a-z0-9][a-z0-9-]{0,19}$ ]]; then
  echo "ERROR: ATTUNE_TEST_RUN_ID must be 1-20 lowercase ASCII letters/digits with optional non-leading '-'" >&2
  exit 2
fi
export ATTUNE_TEST_RUN_ID
DO_BUILD=true
DO_TEARDOWN=true
DO_STARTUP=true
TEST_ARGS=()
RUN_STARTED_NS="$(date +%s%N)"
BUILD_MS=0
STARTUP_MS=0
TEST_MS=0
CLEANUP_MS=0
PEAK_SESSIONS=0
PRE_CLONES=-1
PRE_MIGRATIONS=-1
PRE_TEMPLATES=-1
PRE_SESSIONS=-1
PRE_MIGRATION_SESSIONS=-1
PRE_SCHEMAS=-1
SELECTED_TESTS=-1
SELECTED_SHA256="unknown"
INVENTORY_TESTS=-1
INVENTORY_SHA256="unknown"
CONNECTION_MONITOR_PID=""
CONNECTION_MONITOR_STOP=""
CONNECTION_MONITOR_RESULT=""
TEST_LOG=""

elapsed_ms() {
  echo $((($(date +%s%N) - $1) / 1000000))
}

# ── Parse args ────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-teardown)
      DO_TEARDOWN=false; shift ;;
    --no-build)
      DO_BUILD=false; shift ;;
    --no-startup)
      DO_STARTUP=false; shift ;;
    --crate|-c)
      TEST_ARGS+=("--crate" "$2"); shift 2 ;;
    --test)
      TEST_ARGS+=("--test" "$2"); shift 2 ;;
    --filter|-f)
      TEST_ARGS+=("--filter" "$2"); shift 2 ;;
    -h|--help)
      echo "Usage: $0 [options] [-- cargo-test-args]"
      echo ""
      echo "Options:"
      echo "  --crate, -c <name>  Run tests for a specific crate (common, api, executor, worker)"
      echo "  --test <name>       Run one integration-test executable"
      echo "  --filter, -f <expr> Filter test names"
      echo "  --no-teardown       Keep Docker stack running after tests"
      echo "  --no-build          Skip docker compose build step"
      echo "  --no-startup        Use an explicitly named stack without owning teardown"
      echo "  -- <args>           Extra args passed to cargo test binary"
      echo ""
      echo "Examples:"
      echo "  $0                              # All integration tests"
      echo "  $0 --crate common              # Repository tests only"
      echo "  $0 --crate api                 # API endpoint tests only"
      echo "  $0 --filter test_create_action # Specific test"
      echo "  $0 --no-teardown --crate api   # Keep DB for inspection"
      echo "  $0 -- --nocapture              # Show test stdout"
      exit 0 ;;
    --)
      shift; TEST_ARGS+=("--" "$@"); break ;;
    *)
      TEST_ARGS+=("$1"); shift ;;
  esac
done

compose() {
  docker compose --project-name "$COMPOSE_PROJECT_NAME" "${COMPOSE_FILES[@]}" "$@"
}

query_admin_scalar() {
  compose exec -T postgres psql -X -U attune -d postgres -qAt -c "$1"
}

start_connection_monitor() {
  [[ -n "${ATTUNE_BENCHMARK_OUTPUT:-}" ]] || return 0
  local run_token="${ATTUNE_TEST_RUN_ID//-/_}"
  local database_prefix="attune_db_${run_token}_"
  local migration_prefix="attune_migration_${run_token}_"
  CONNECTION_MONITOR_STOP="/tmp/attune-benchmark-stop-${COMPOSE_PROJECT_NAME}-$$"
  CONNECTION_MONITOR_RESULT="/tmp/attune-benchmark-peak-${COMPOSE_PROJECT_NAME}-$$"
  rm -f "$CONNECTION_MONITOR_STOP" "$CONNECTION_MONITOR_RESULT"
  (
    local peak=0 current
    while [[ ! -e "$CONNECTION_MONITOR_STOP" ]]; do
      current="$(query_admin_scalar \
        "SELECT count(*) FROM pg_stat_activity WHERE pid <> pg_backend_pid() AND (datname = 'attune_test' OR left(datname, ${#database_prefix}) = '${database_prefix}' OR left(datname, ${#migration_prefix}) = '${migration_prefix}');" \
        2>/dev/null || true)"
      if [[ "$current" =~ ^[0-9]+$ ]] && ((current > peak)); then
        peak=$current
      fi
      sleep 1
    done
    echo "$peak" > "$CONNECTION_MONITOR_RESULT"
  ) &
  CONNECTION_MONITOR_PID=$!
}

stop_connection_monitor() {
  [[ -n "$CONNECTION_MONITOR_PID" ]] || return 0
  touch "$CONNECTION_MONITOR_STOP"
  wait "$CONNECTION_MONITOR_PID" || true
  if [[ -s "$CONNECTION_MONITOR_RESULT" ]]; then
    PEAK_SESSIONS="$(<"$CONNECTION_MONITOR_RESULT")"
  fi
  rm -f "$CONNECTION_MONITOR_STOP" "$CONNECTION_MONITOR_RESULT"
  CONNECTION_MONITOR_PID=""
}

collect_pre_teardown_metrics() {
  [[ -n "${ATTUNE_BENCHMARK_OUTPUT:-}" ]] || return 0
  local run_token="${ATTUNE_TEST_RUN_ID//-/_}"
  local database_prefix="attune_db_${run_token}_"
  local migration_prefix="attune_migration_${run_token}_"
  local template_prefix="attune_tpl_${run_token}_"
  local schema_prefix="test_${run_token}_"
  PRE_CLONES="$(query_admin_scalar "SELECT count(*) FROM pg_database WHERE left(datname, ${#database_prefix}) = '${database_prefix}';")" || return
  PRE_MIGRATIONS="$(query_admin_scalar "SELECT count(*) FROM pg_database WHERE left(datname, ${#migration_prefix}) = '${migration_prefix}';")" || return
  PRE_TEMPLATES="$(query_admin_scalar "SELECT count(*) FROM pg_database WHERE left(datname, ${#template_prefix}) = '${template_prefix}';")" || return
  PRE_SESSIONS="$(query_admin_scalar "SELECT count(*) FROM pg_stat_activity WHERE left(datname, ${#database_prefix}) = '${database_prefix}';")" || return
  PRE_MIGRATION_SESSIONS="$(query_admin_scalar "SELECT count(*) FROM pg_stat_activity WHERE left(datname, ${#migration_prefix}) = '${migration_prefix}';")" || return
  PRE_SCHEMAS="$(query_admin_scalar "SELECT count(*) FROM pg_namespace WHERE left(nspname, ${#schema_prefix}) = '${schema_prefix}';")" || return
}

write_benchmark_record() {
  [[ -n "${ATTUNE_BENCHMARK_OUTPUT:-}" ]] || return 0
  local exit_code="$1"
  local total_ms
  total_ms="$(elapsed_ms "$RUN_STARTED_NS")"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "${ATTUNE_BENCHMARK_SAMPLE:-unspecified}" \
    "${ATTUNE_BENCHMARK_MODE:-unspecified}" \
    "$ATTUNE_TEST_RUN_ID" \
    "${ATTUNE_RUST_TEST_THREADS:-1}" \
    "$SELECTED_TESTS" "$SELECTED_SHA256" "$INVENTORY_TESTS" "$INVENTORY_SHA256" \
    "$BUILD_MS" "$STARTUP_MS" "$TEST_MS" "$CLEANUP_MS" "$total_ms" \
    "$exit_code" "$PEAK_SESSIONS" "$PRE_CLONES" "$PRE_MIGRATIONS" "$PRE_TEMPLATES" \
    "$PRE_SESSIONS" "$PRE_MIGRATION_SESSIONS" "$PRE_SCHEMAS" \
    >> "$ATTUNE_BENCHMARK_OUTPUT"
}

STACK_STARTED=false
assert_fresh_project() {
  local containers volumes networks
  if ! containers="$(compose ps -aq 2>/dev/null)" ||
     ! volumes="$(docker volume ls -q --filter "label=com.docker.compose.project=${COMPOSE_PROJECT_NAME}")" ||
     ! networks="$(docker network ls -q --filter "label=com.docker.compose.project=${COMPOSE_PROJECT_NAME}")"; then
    echo -e "${RED}ERROR: could not verify project freshness for '${COMPOSE_PROJECT_NAME}'${NC}" >&2
    return 1
  fi
  if [[ -n "$containers" || -n "$volumes" || -n "$networks" ]]; then
    echo -e "${RED}ERROR: refusing to start in existing project '${COMPOSE_PROJECT_NAME}'${NC}" >&2
    return 1
  fi
}

if [[ "$DO_STARTUP" == false && -z "${ATTUNE_E2E_PROJECT_NAME:-}" ]]; then
  echo "ERROR: --no-startup requires ATTUNE_E2E_PROJECT_NAME naming the existing disposable stack" >&2
  exit 2
fi
if [[ "$DO_STARTUP" == false && "$DO_BUILD" == true ]]; then
  echo "ERROR: --no-startup also requires --no-build so the attached project cannot be mutated" >&2
  exit 2
fi

# ── Cleanup trap ─────────────────────────────────────────────────────────
cleanup() {
  local exit_code=$?
  local cleanup_code=0
  local cleanup_started_ns
  stop_connection_monitor
  [[ -z "$TEST_LOG" ]] || rm -f "$TEST_LOG"
  if [[ "$STACK_STARTED" == true || "$DO_STARTUP" == false ]]; then
    collect_pre_teardown_metrics || {
      cleanup_code=$?
      echo -e "${RED}ERROR: benchmark resource inspection failed (exit ${cleanup_code})${NC}" >&2
      [[ $exit_code -ne 0 ]] || exit_code=$cleanup_code
    }
  fi
  if [[ "$DO_TEARDOWN" == true && "$STACK_STARTED" == true ]]; then
    echo -e "\n${CYAN}Tearing down owned project '${COMPOSE_PROJECT_NAME}'...${NC}"
    cleanup_started_ns="$(date +%s%N)"
    compose down --remove-orphans --timeout 10 --volumes || cleanup_code=$?
    CLEANUP_MS="$(elapsed_ms "$cleanup_started_ns")"
    if [[ $cleanup_code -ne 0 ]]; then
      echo -e "${RED}ERROR: owned stack teardown failed (exit ${cleanup_code})${NC}" >&2
      [[ $exit_code -ne 0 ]] || exit_code=$cleanup_code
    fi
  elif [[ "$DO_STARTUP" == false ]]; then
    echo -e "\n${YELLOW}Attached stack was not torn down; this invocation did not start it.${NC}"
  elif [[ "$STACK_STARTED" == true ]]; then
    echo -e "\n${YELLOW}Owned stack '${COMPOSE_PROJECT_NAME}' left running (--no-teardown).${NC}"
  else
    echo -e "\n${YELLOW}No stack was started; no teardown was needed.${NC}"
  fi
  write_benchmark_record "$exit_code"
  trap - EXIT
  exit $exit_code
}
trap cleanup EXIT

if [[ "$DO_STARTUP" == true ]]; then
  # Check ownership before building: Compose image tags are project-scoped, so
  # even a build could mutate an existing project's next startup behavior.
  assert_fresh_project
fi

# ── Build ────────────────────────────────────────────────────────────────
if [[ "$DO_BUILD" == true ]]; then
  echo -e "${CYAN}Building rust-int-tests container...${NC}"
  phase_started_ns="$(date +%s%N)"
  compose build rust-int-tests
  BUILD_MS="$(elapsed_ms "$phase_started_ns")"
fi

# ── Start infrastructure ─────────────────────────────────────────────────
if [[ "$DO_STARTUP" == true ]]; then
  phase_started_ns="$(date +%s%N)"
  STACK_STARTED=true
  echo -e "${CYAN}Starting PostgreSQL and RabbitMQ for run '${RUN_ID}'...${NC}"
  compose up -d postgres rabbitmq
  
  # pg_isready also succeeds against the temporary server used by the image's
  # initialization scripts. PID 1 becomes postgres only after that server has
  # stopped and the final server has started.
  echo -e "${CYAN}Waiting for final PostgreSQL server to become healthy...${NC}"
  local_wait=0
  while [[ $local_wait -lt 60 ]]; do
    if compose exec -T postgres sh -c 'test "$(cat /proc/1/comm)" = postgres' >/dev/null 2>&1 &&
       compose exec -T postgres pg_isready -U attune >/dev/null 2>&1; then
      break
    fi
    sleep 1
    local_wait=$((local_wait + 1))
  done
  
  if [[ $local_wait -ge 60 ]]; then
    echo -e "${RED}ERROR: Postgres failed to become healthy in 60s${NC}" >&2
    exit 1
  fi
  echo -e "${GREEN}Postgres ready (${local_wait}s)${NC}"

  echo -e "${CYAN}Waiting for RabbitMQ to become healthy...${NC}"
  local_wait=0
  while [[ $local_wait -lt 90 ]]; do
    if compose exec -T rabbitmq rabbitmq-diagnostics -q ping >/dev/null 2>&1; then
      break
    fi
    sleep 1
    local_wait=$((local_wait + 1))
  done
  if [[ $local_wait -ge 90 ]]; then
    echo -e "${RED}ERROR: RabbitMQ failed to become healthy in 90s${NC}" >&2
    exit 1
  fi
  echo -e "${GREEN}RabbitMQ ready (${local_wait}s)${NC}"

  # Ensure attune_test database exists
  echo -e "${CYAN}Ensuring attune_test database exists...${NC}"
  compose exec -T postgres \
    psql -U attune -d postgres -c "SELECT 1 FROM pg_database WHERE datname = 'attune_test'" | grep -q 1 || \
  compose exec -T postgres \
    psql -U attune -d postgres -c "CREATE DATABASE attune_test OWNER attune TEMPLATE template0;"
  echo -e "${GREEN}Database ready${NC}"
  STARTUP_MS="$(elapsed_ms "$phase_started_ns")"
fi

# ── Run tests ────────────────────────────────────────────────────────────
echo -e "\n${CYAN}Running Rust integration tests...${NC}\n"

start_connection_monitor
phase_started_ns="$(date +%s%N)"
if [[ -n "${ATTUNE_BENCHMARK_OUTPUT:-}" ]]; then
  TEST_LOG="$(mktemp)"
fi
set +e
if [[ -n "$TEST_LOG" ]]; then
  if [[ ${#TEST_ARGS[@]} -gt 0 ]]; then
    compose run --rm --no-deps rust-int-tests "${TEST_ARGS[@]}" 2>&1 | tee "$TEST_LOG"
  else
    compose run --rm --no-deps rust-int-tests 2>&1 | tee "$TEST_LOG"
  fi
  EXIT_CODE=${PIPESTATUS[0]}
elif [[ ${#TEST_ARGS[@]} -gt 0 ]]; then
  compose run --rm --no-deps rust-int-tests "${TEST_ARGS[@]}"
  EXIT_CODE=$?
else
  compose run --rm --no-deps rust-int-tests
  EXIT_CODE=$?
fi
set -e
TEST_MS="$(elapsed_ms "$phase_started_ns")"
stop_connection_monitor

if [[ -n "$TEST_LOG" ]]; then
  selection_line="$(grep -m1 '^ATTUNE_TEST_SELECTION ' "$TEST_LOG" || true)"
  rm -f "$TEST_LOG"
  TEST_LOG=""
  if [[ "$selection_line" =~ selected=([0-9]+)[[:space:]]+selected_sha256=([0-9a-f]{64})[[:space:]]+inventoried=([0-9]+)[[:space:]]+inventory_sha256=([0-9a-f]{64})$ ]]; then
    SELECTED_TESTS="${BASH_REMATCH[1]}"
    SELECTED_SHA256="${BASH_REMATCH[2]}"
    INVENTORY_TESTS="${BASH_REMATCH[3]}"
    INVENTORY_SHA256="${BASH_REMATCH[4]}"
  else
    echo -e "${RED}ERROR: benchmark selection metadata was not reported${NC}" >&2
    [[ $EXIT_CODE -ne 0 ]] || EXIT_CODE=2
  fi
fi

# ── Report ───────────────────────────────────────────────────────────────
echo ""
if [[ $EXIT_CODE -eq 0 ]]; then
  echo -e "${GREEN}╔════════════════════════════════════════════════════════╗${NC}"
  echo -e "${GREEN}║  ✓ Rust integration tests passed                     ║${NC}"
  echo -e "${GREEN}╚════════════════════════════════════════════════════════╝${NC}"
else
  echo -e "${RED}╔════════════════════════════════════════════════════════╗${NC}"
  echo -e "${RED}║  ✗ Rust integration tests failed (exit code: $EXIT_CODE)     ║${NC}"
  echo -e "${RED}║                                                        ║${NC}"
  echo -e "${RED}║  Re-run with --no-teardown to inspect the DB:          ║${NC}"
  echo -e "${RED}║    make rust-int-test-debug ARGS='--crate common'      ║${NC}"
  echo -e "${RED}╚════════════════════════════════════════════════════════╝${NC}"
fi

exit $EXIT_CODE
