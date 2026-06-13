#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BENCHMARK_PY="$PROJECT_ROOT/tests/benchmarks/metadata_cache_benchmark.py"
VENV_DIR="$PROJECT_ROOT/tests/venvs/benchmark"
DEFAULT_RESULTS_DIR="$PROJECT_ROOT/benchmark-results/metadata-cache-$(date +%Y%m%d-%H%M%S)"
BENCHMARK_PYTHON="${BENCHMARK_PYTHON:-python3.13}"
MODE="both"
RESULTS_DIR="$DEFAULT_RESULTS_DIR"
SKIP_BUILD=false
KEEP_UP=false
SETUP_ONLY=false
EXTRA_ARGS=()

usage() {
  cat <<'EOF'
Usage: ./scripts/run-metadata-cache-benchmark.sh [options] [-- benchmark-args]

Runs the blended metadata-cache benchmark in Docker Compose with cache-on and/or
cache-off modes, captures raw JSON results, and writes a Markdown comparison.

Options:
  --mode MODE         one of: both, cache-on, cache-off (default: both)
  --results-dir DIR   output directory (default: benchmark-results/metadata-cache-<timestamp>)
  --skip-build        use existing images/containers without --build
  --keep-up           leave the final benchmark environment running
  --setup-only        create/update the benchmark virtualenv and exit
  -h, --help          show this help text

Any arguments after -- are forwarded to metadata_cache_benchmark.py run.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)
      MODE="$2"
      shift 2
      ;;
    --results-dir)
      RESULTS_DIR="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=true
      shift
      ;;
    --keep-up)
      KEEP_UP=true
      shift
      ;;
    --setup-only)
      SETUP_ONLY=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      EXTRA_ARGS=("$@")
      break
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

mkdir -p "$RESULTS_DIR"

setup_venv() {
  if ! command -v "$BENCHMARK_PYTHON" >/dev/null 2>&1; then
    echo "Required benchmark interpreter not found: $BENCHMARK_PYTHON" >&2
    echo "Set BENCHMARK_PYTHON to an installed Python 3.13 binary if needed." >&2
    exit 1
  fi

  local recreate=false
  if [[ -d "$VENV_DIR" ]]; then
    local venv_version
    venv_version="$("$VENV_DIR/bin/python" -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')"
    if [[ "$venv_version" != "3.13" ]]; then
      recreate=true
    fi
  fi

  if [[ ! -d "$VENV_DIR" || "$recreate" == true ]]; then
    rm -rf "$VENV_DIR"
    "$BENCHMARK_PYTHON" -m venv "$VENV_DIR"
  fi
  # shellcheck disable=SC1090
  source "$VENV_DIR/bin/activate"
  python -m pip install -q --upgrade pip
  python -m pip install -q -r "$PROJECT_ROOT/tests/requirements.txt"
}

compose_cmd() {
  local mode="$1"
  shift
  local args=(-f "$PROJECT_ROOT/docker-compose.yaml")
  if [[ "$mode" == "cache-off" ]]; then
    args+=(-f "$PROJECT_ROOT/docker/compose.metadata-cache-off.yaml")
  fi
  docker compose "${args[@]}" "$@"
}

wait_for_api() {
  local attempts=150
  for ((i=1; i<=attempts; i++)); do
    if curl -fsS http://localhost:8080/health >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  return 1
}

teardown_mode() {
  local mode="$1"
  compose_cmd "$mode" down -v --remove-orphans --timeout 15 >/dev/null 2>&1 || true
}

start_mode() {
  local mode="$1"
  local up_args=(up -d)
  if [[ "$SKIP_BUILD" == false ]]; then
    up_args+=(--build)
  fi
  compose_cmd "$mode" "${up_args[@]}"
  if ! wait_for_api; then
    echo "API did not become healthy for $mode" >&2
    compose_cmd "$mode" logs --tail=100 api executor executor-2 worker-shell worker-full || true
    return 1
  fi
}

run_single_mode() {
  local mode="$1"
  local output_json="$RESULTS_DIR/${mode}.json"
  local benchmark_cmd=(
    python "$BENCHMARK_PY" run
    --mode "$mode"
    --output "$output_json"
  )

  echo "=== Running benchmark in ${mode} mode ==="
  teardown_mode "$mode"
  start_mode "$mode"

  # shellcheck disable=SC1090
  source "$VENV_DIR/bin/activate"
  if ((${#EXTRA_ARGS[@]} > 0)); then
    benchmark_cmd+=("${EXTRA_ARGS[@]}")
  fi
  "${benchmark_cmd[@]}"

  if [[ "$KEEP_UP" == false ]]; then
    teardown_mode "$mode"
  fi
}

setup_venv

if [[ "$SETUP_ONLY" == true ]]; then
  echo "Benchmark virtualenv ready at $VENV_DIR"
  exit 0
fi

trap 'if [[ "$KEEP_UP" == false ]]; then teardown_mode cache-on; teardown_mode cache-off; fi' EXIT

case "$MODE" in
  both)
    run_single_mode cache-on
    run_single_mode cache-off
    # shellcheck disable=SC1090
    source "$VENV_DIR/bin/activate"
    python "$BENCHMARK_PY" compare \
      --cache-on "$RESULTS_DIR/cache-on.json" \
      --cache-off "$RESULTS_DIR/cache-off.json" \
      --output "$RESULTS_DIR/comparison.md" \
      --json-output "$RESULTS_DIR/comparison.json"
    ;;
  cache-on|cache-off)
    run_single_mode "$MODE"
    ;;
  *)
    echo "Invalid mode: $MODE" >&2
    exit 1
    ;;
esac

echo "Benchmark outputs written to $RESULTS_DIR"
