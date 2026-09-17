#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf -- "$TMP_ROOT"' EXIT

expect_exit() {
  local expected="$1"
  shift
  set +e
  "$@" >"$TMP_ROOT/stdout" 2>"$TMP_ROOT/stderr"
  local actual=$?
  set -e
  if [[ "$actual" -ne "$expected" ]]; then
    echo "expected exit $expected, got $actual: $*" >&2
    cat "$TMP_ROOT/stdout" "$TMP_ROOT/stderr" >&2
    exit 1
  fi
}

expect_exit 2 env ATTUNE_TEST_RUN_ID=a_b bash "$ROOT/scripts/cleanup-test-schemas.sh" --force
expect_exit 2 env ATTUNE_E2E_PROJECT_NAME= bash "$ROOT/scripts/run-integration-tests.sh" --no-startup --no-build
expect_exit 2 env ATTUNE_E2E_PROJECT_NAME= bash "$ROOT/scripts/run-rust-integration-tests.sh" --no-startup --no-build
expect_exit 2 env ATTUNE_E2E_RUN_ID=guard-attach ATTUNE_E2E_PROJECT_NAME=attune-guard-attach \
  bash "$ROOT/scripts/run-integration-tests.sh" --no-startup
expect_exit 2 env ATTUNE_E2E_RUN_ID=guard-attach ATTUNE_E2E_PROJECT_NAME=attune-guard-attach \
  bash "$ROOT/scripts/run-rust-integration-tests.sh" --no-startup

mkdir "$TMP_ROOT/bin"
cat > "$TMP_ROOT/bin/psql" <<'EOF'
#!/usr/bin/env sh
exit 7
EOF
chmod +x "$TMP_ROOT/bin/psql"
expect_exit 7 env PATH="$TMP_ROOT/bin:$PATH" ATTUNE_TEST_RUN_ID=guard1 \
  DATABASE_URL=postgresql://example.invalid/attune_test \
  bash "$ROOT/scripts/cleanup-test-schemas.sh" --force

echo "test isolation guard checks passed"
