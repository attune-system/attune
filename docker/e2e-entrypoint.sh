#!/usr/bin/env bash
#
# Entrypoint for the E2E test runner container.
#
# Waits for the API to be healthy, then runs pytest with any
# arguments passed to the container.
#
# Supported arguments (passed through to the run_e2e.sh wrapper or pytest):
#   --tier <N>        Run only tier N tests (1, 2, or 3)
#   --tier1 / --tier2 / --tier3   Shorthand for --tier
#   -k <EXPR>         Pytest filter expression
#   -m <MARKER>       Pytest marker filter
#   -x                Stop on first failure
#   -v                Verbose output
#   --html <path>     Generate HTML report
#   Any other pytest arguments are passed through directly.
#
set -e

API_URL="${ATTUNE_API_URL:-http://api:8080}"
MAX_WAIT="${E2E_MAX_WAIT:-120}"

sanitize_url_origin() {
  local url=$1 scheme authority
  case "$url" in
    http://*) scheme=http; authority=${url#http://} ;;
    https://*) scheme=https; authority=${url#https://} ;;
    *) printf '%s\n' '<url configured>'; return ;;
  esac
  authority=${authority%%/*}
  authority=${authority%%\?*}
  authority=${authority%%\#*}
  case "$authority" in
    *@*@*|'') printf '%s\n' '<url configured>'; return ;;
    *@*) authority=${authority#*@} ;;
  esac
  case "$authority" in
    ''|*[[:space:]]*|*\\*) printf '%s\n' '<url configured>'; return ;;
  esac
  case "$authority" in
    \[*\])
      local display_host=${authority#\[}; display_host=${display_host%\]}
      case "$display_host" in ''|*[!0-9A-Fa-f:.]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    \[*\]:*)
      local display_host=${authority#\[} display_port
      display_port=${display_host#*\]}; display_host=${display_host%%\]*}; display_port=${display_port#:}
      case "$display_host" in ''|*[!0-9A-Fa-f:.]*) printf '%s\n' '<url configured>'; return ;; esac
      case "$display_port" in ''|*[!0-9]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    *:*)
      local display_host=${authority%:*} display_port=${authority##*:}
      case "$display_host" in ''|*:*|*[!A-Za-z0-9._~-]*) printf '%s\n' '<url configured>'; return ;; esac
      case "$display_port" in ''|*[!0-9]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    *) case "$authority" in *[!A-Za-z0-9._~-]*) printf '%s\n' '<url configured>'; return ;; esac ;;
  esac
  printf '%s://%s\n' "$scheme" "$authority"
}

API_URL_ORIGIN=$(sanitize_url_origin "$API_URL")

# ── Wait for API ──────────────────────────────────────────────────────────
echo "⏳ Waiting for API at ${API_URL_ORIGIN} (timeout: ${MAX_WAIT}s)..."
elapsed=0
while ! curl -sf "${API_URL}/health" > /dev/null 2>&1; do
  if [ "$elapsed" -ge "$MAX_WAIT" ]; then
    echo "✗ API did not become healthy within ${MAX_WAIT}s"
    exit 1
  fi
  sleep 2
  elapsed=$((elapsed + 2))
done
echo "✓ API is healthy (waited ${elapsed}s)"

# ── Parse arguments ───────────────────────────────────────────────────────
PYTEST_ARGS=()
TEST_PATHS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tier)
      TEST_PATHS+=("tests/e2e/tier${2}/")
      shift 2
      ;;
    --tier1)
      TEST_PATHS+=("tests/e2e/tier1/")
      shift
      ;;
    --tier2)
      TEST_PATHS+=("tests/e2e/tier2/")
      shift
      ;;
    --tier3)
      TEST_PATHS+=("tests/e2e/tier3/")
      shift
      ;;
    *)
      PYTEST_ARGS+=("$1")
      shift
      ;;
  esac
done

# Default to all tiers if none specified
if [ ${#TEST_PATHS[@]} -eq 0 ]; then
  TEST_PATHS=("tests/e2e/")
fi

# ── Run tests ─────────────────────────────────────────────────────────────
cd /app

export PYTHONPATH="/app/tests:/app:${PYTHONPATH:-}"

echo ""
echo "╔════════════════════════════════════════════════════════╗"
echo "║  Attune E2E Integration Tests                         ║"
echo "╚════════════════════════════════════════════════════════╝"
echo ""
echo "  API origin: ${API_URL_ORIGIN}"
echo "  Path:  ${TEST_PATHS[*]}"
echo "  Args:  ${PYTEST_ARGS[*]:-<none>}"
echo ""

exec pytest "${TEST_PATHS[@]}" "${PYTEST_ARGS[@]}"
