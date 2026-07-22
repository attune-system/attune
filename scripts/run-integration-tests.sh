#!/usr/bin/env bash
#
# Full-lifecycle E2E integration test runner.
#
# Orchestrates the complete cycle:
#   1. Start the Docker Compose stack (if not already running)
#   2. Wait for all services to become healthy
#   3. Run pytest inside the e2e-tests container
#   4. Optionally tear down the stack
#
# Usage:
#   ./scripts/run-integration-tests.sh              # Run all tiers
#   ./scripts/run-integration-tests.sh --tier 1     # Run tier 1 only
#   ./scripts/run-integration-tests.sh --no-teardown  # Keep stack running after tests
#   ./scripts/run-integration-tests.sh --no-build   # Skip docker build step
#   ./scripts/run-integration-tests.sh -k "timer"   # Filter by expression
#   ./scripts/run-integration-tests.sh --help       # Show help
#
set -euo pipefail

# ── Colours ───────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# ── Defaults ──────────────────────────────────────────────────────────────
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILES=("-f" "$PROJECT_ROOT/docker-compose.yaml" "-f" "$PROJECT_ROOT/docker-compose.e2e.yaml")
DO_BUILD=true
DO_TEARDOWN=true
DO_STARTUP=true
DO_STANDALONE=false
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
    --standalone)
      DO_STANDALONE=true; shift ;;
    -m)
      shift
      MARKER_PARTS=()
      while [[ $# -gt 0 && "$1" != -* ]]; do
        MARKER_PARTS+=("$1")
        shift
      done
      if [[ ${#MARKER_PARTS[@]} -eq 0 ]]; then
        echo "ERROR: -m requires a pytest marker expression" >&2
        exit 2
      fi
      TEST_ARGS+=("-m" "${MARKER_PARTS[*]}") ;;
    -h|--help)
      echo "Usage: $0 [options] [-- pytest-args...]"
      echo ""
      echo "Options:"
      echo "  --tier <N>        Run tier N only (1, 2, 3)"
      echo "  --no-teardown     Keep Docker stack running after tests"
      echo "  --no-build        Skip docker compose build step"
      echo "  --no-startup      Skip stack startup (assume it's already running)"
      echo "  --standalone    Include standalone worker/sensor services"
      echo "  -k <EXPR>         Pytest filter expression"
      echo "  -m <MARKER>       Pytest marker filter"
      echo "  -x                Stop on first failure"
      echo "  -h, --help        Show this help"
      echo ""
      echo "Examples:"
      echo "  $0                         # Run all tiers"
      echo "  $0 --tier 1                # Run tier 1 only"
      echo "  $0 --no-teardown --tier 2  # Run tier 2, keep stack"
      echo "  $0 -k 'timer' -x          # Filter + stop on first failure"
      echo "  $0 --standalone -k standalone  # Run standalone transport tests"
      exit 0
      ;;
    *)
      TEST_ARGS+=("$1"); shift ;;
  esac
done

cd "$PROJECT_ROOT"

# Performance scenarios deliberately generate large datasets and WAL volume.
# Keep them opt-in, while allowing an explicit `-m "... performance ..."` marker
# expression (such as make e2e-test-cache-load) to select them. Extend an
# existing expression rather than adding a second -m option, because pytest
# treats repeated marker options as replacement rather than intersection.
MARKER_FOUND=false
for ((i = 0; i < ${#TEST_ARGS[@]}; i++)); do
  if [[ "${TEST_ARGS[$i]}" == "-m" ]] && ((i + 1 < ${#TEST_ARGS[@]})); then
    MARKER_FOUND=true
    marker_index=$((i + 1))
    marker_expression="${TEST_ARGS[$marker_index]}"
    if [[ "$marker_expression" != *"performance"* ]]; then
      TEST_ARGS[$marker_index]="(${marker_expression}) and not performance"
    fi
    ((i++))
  fi
done
if [[ "$MARKER_FOUND" == false ]]; then
  TEST_ARGS+=("-m" "not performance")
fi

# Add standalone compose file if requested
if $DO_STANDALONE; then
  COMPOSE_FILES+=("-f" "$PROJECT_ROOT/docker-compose.standalone.yaml")
fi

log_info()    { echo -e "${BLUE}ℹ${NC}  $1"; }
log_success() { echo -e "${GREEN}✓${NC}  $1"; }
log_warn()    { echo -e "${YELLOW}⚠${NC}  $1"; }
log_error()   { echo -e "${RED}✗${NC}  $1"; }
log_header()  { echo -e "${CYAN}═══${NC} $1"; }

compose() {
  docker compose "${COMPOSE_FILES[@]}" "$@"
}

wait_for_registered_worker() {
  local name_substring="$1"
  local max_wait="${2:-120}"
  local elapsed=0
  local count=0

  log_info "Waiting for worker containing '${name_substring}' to register..."
  while true; do
    count="$(
      compose exec -T postgres psql -U attune -d attune -tAc \
        "SELECT COUNT(*) FROM worker WHERE name LIKE '%${name_substring}%' AND status IN ('active', 'busy')" \
        2>/dev/null | tr -d '[:space:]'
    )" || count=0

    if [[ "${count:-0}" =~ ^[0-9]+$ ]] && [[ "$count" -ge 1 ]]; then
      log_success "Standalone worker registered (${elapsed}s)"
      return 0
    fi

    if [[ "$elapsed" -ge "$max_wait" ]]; then
      log_error "Standalone worker did not register within ${max_wait}s"
      compose logs --tail=80 worker-standalone sensor-standalone || true
      return 1
    fi

    sleep 3
    elapsed=$((elapsed + 3))
  done
}

# ── Cleanup handler ───────────────────────────────────────────────────────
cleanup() {
  local exit_code=$?
  if [[ "$DO_TEARDOWN" == true ]]; then
    echo ""
    log_info "Tearing down Docker stack..."
    compose down --timeout 15 --volumes 2>/dev/null || true
  else
    log_info "Stack left running (--no-teardown). Tear down with: make docker-down"
  fi
  exit $exit_code
}
trap cleanup EXIT

# ── Step 1: Build ─────────────────────────────────────────────────────────
if [[ "$DO_BUILD" == true ]]; then
  log_header "Building Docker images..."
  compose build --quiet \
    e2e-tests \
    migrations init-user init-pack-binaries init-packs init-agent \
    api executor executor-2 notifier supervisor
  log_success "Build complete"
fi

# ── Step 2: Start stack ───────────────────────────────────────────────────
if [[ "$DO_STARTUP" == true ]]; then
  log_header "Starting Attune services..."
  SERVICES=(
    postgres rabbitmq
    migrations init-user init-pack-binaries init-packs init-agent
    api executor executor-2 worker-shell worker-python worker-node worker-full
    sensor notifier
  )
  if [[ "$DO_STANDALONE" == true ]]; then
    SERVICES+=(worker-standalone sensor-standalone)
  fi

  # Start infrastructure + application services (not e2e-tests — that's run separately)
  compose up -d --no-deps "${SERVICES[@]}"

  # Wait for API health check
  log_info "Waiting for API to become healthy..."
  max_wait=180
  elapsed=0
  while ! curl -sf http://localhost:8080/health > /dev/null 2>&1; do
    if [ "$elapsed" -ge "$max_wait" ]; then
      log_error "API did not become healthy within ${max_wait}s"
      compose logs --tail=50 api
      exit 1
    fi
    sleep 3
    elapsed=$((elapsed + 3))
  done
  log_success "API healthy (${elapsed}s)"

  # Configure the ephemeral stack's short cache lifecycle windows through the
  # public API before starting the supervisor. This keeps production defaults
  # intact while allowing cursor-expiry and bounded-cleanup scenarios to poll
  # deterministically instead of sleeping for the production retention window.
  log_info "Configuring E2E cache retention..."
  compose run --rm --entrypoint python3 \
    -e PYTHONPATH=/app/tests:/app \
    e2e-tests \
    /app/tests/e2e/configure_cache_retention.py
  compose up -d --no-deps supervisor

  # Brief pause for executor/worker registration
  log_info "Waiting for workers to register..."
  sleep 5
  if [[ "$DO_STANDALONE" == true ]]; then
    wait_for_registered_worker "standalone" 180
  fi
  log_success "Stack ready"
fi

# ── Step 3: Run tests ────────────────────────────────────────────────────
echo ""
log_header "Running E2E integration tests..."
echo ""

# Run the test container (exits with pytest's exit code)
# Use `run --rm` so the container is removed after tests finish
TEST_EXIT_CODE=0
if [[ ${#TEST_ARGS[@]} -gt 0 ]]; then
  compose run --rm e2e-tests "${TEST_ARGS[@]}" || TEST_EXIT_CODE=$?
else
  compose run --rm e2e-tests || TEST_EXIT_CODE=$?
fi

# ── Step 4: Report ────────────────────────────────────────────────────────
echo ""
echo "╔════════════════════════════════════════════════════════╗"
if [[ $TEST_EXIT_CODE -eq 0 ]]; then
  echo -e "║  ${GREEN}✓ All E2E tests passed${NC}                                ║"
else
  echo -e "║  ${RED}✗ E2E tests failed (exit code: ${TEST_EXIT_CODE})${NC}                  ║"
  echo "║                                                        ║"
  echo "║  Re-run with --no-teardown to inspect the stack:       ║"
  echo "║    make e2e-test-debug ARGS='--tier 1 -x'              ║"
fi
echo "╚════════════════════════════════════════════════════════╝"
echo ""

exit $TEST_EXIT_CODE
