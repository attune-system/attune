#!/usr/bin/env bash
#
# Orchestration script for Rust integration tests in Docker.
#
# Starts the database (and optionally full stack), builds the Rust test
# container, runs the #[ignore]'d integration tests, and tears down.
#
# Usage:
#   ./scripts/run-rust-integration-tests.sh              # All crates
#   ./scripts/run-rust-integration-tests.sh --crate common  # Specific crate
#   ./scripts/run-rust-integration-tests.sh --crate api     # API tests
#   ./scripts/run-rust-integration-tests.sh --filter test_create_action
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
DO_BUILD=true
DO_TEARDOWN=true
DO_STARTUP=true
TEST_ARGS=()

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
    --filter|-f)
      TEST_ARGS+=("--filter" "$2"); shift 2 ;;
    -h|--help)
      echo "Usage: $0 [options] [-- cargo-test-args]"
      echo ""
      echo "Options:"
      echo "  --crate, -c <name>  Run tests for a specific crate (common, api, executor, worker)"
      echo "  --filter, -f <expr> Filter test names"
      echo "  --no-teardown       Keep Docker stack running after tests"
      echo "  --no-build          Skip docker compose build step"
      echo "  --no-startup        Skip stack startup (assume it's already running)"
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

# ── Cleanup trap ─────────────────────────────────────────────────────────
cleanup() {
  local exit_code=$?
  if [[ "$DO_TEARDOWN" == true ]]; then
    echo -e "\n${CYAN}Tearing down...${NC}"
    docker compose "${COMPOSE_FILES[@]}" down --remove-orphans --timeout 10 2>/dev/null || true
  else
    echo -e "\n${YELLOW}Stack left running (--no-teardown).${NC}"
    echo -e "  Tear down manually: docker compose ${COMPOSE_FILES[*]} down"
  fi
  exit $exit_code
}
trap cleanup EXIT

# ── Build ────────────────────────────────────────────────────────────────
if [[ "$DO_BUILD" == true ]]; then
  echo -e "${CYAN}Building rust-int-tests container...${NC}"
  docker compose "${COMPOSE_FILES[@]}" build rust-int-tests
fi

# ── Start infrastructure ─────────────────────────────────────────────────
if [[ "$DO_STARTUP" == true ]]; then
  echo -e "${CYAN}Starting database...${NC}"
  docker compose "${COMPOSE_FILES[@]}" up -d postgres
  
  # Wait for postgres to be healthy
  echo -e "${CYAN}Waiting for postgres to become healthy...${NC}"
  local_wait=0
  while [[ $local_wait -lt 60 ]]; do
    if docker compose "${COMPOSE_FILES[@]}" exec -T postgres pg_isready -U attune >/dev/null 2>&1; then
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

  # Ensure attune_test database exists
  echo -e "${CYAN}Ensuring attune_test database exists...${NC}"
  docker compose "${COMPOSE_FILES[@]}" exec -T postgres \
    psql -U attune -d postgres -c "SELECT 1 FROM pg_database WHERE datname = 'attune_test'" | grep -q 1 || \
  docker compose "${COMPOSE_FILES[@]}" exec -T postgres \
    psql -U attune -d postgres -c "CREATE DATABASE attune_test OWNER attune;" 2>/dev/null || true
  echo -e "${GREEN}Database ready${NC}"
fi

# ── Run tests ────────────────────────────────────────────────────────────
echo -e "\n${CYAN}Running Rust integration tests...${NC}\n"

set +e
if [[ ${#TEST_ARGS[@]} -gt 0 ]]; then
  docker compose "${COMPOSE_FILES[@]}" run --rm rust-int-tests "${TEST_ARGS[@]}"
else
  docker compose "${COMPOSE_FILES[@]}" run --rm rust-int-tests
fi
EXIT_CODE=$?
set -e

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
