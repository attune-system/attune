#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
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
expect_exit 2 bash "$ROOT/scripts/check-db-test-threads.sh" 3
bash "$ROOT/scripts/check-db-test-threads.sh" 4
bash "$ROOT/scripts/database-test-ignore-policy.sh" --check
expect_exit 2 env ATTUNE_RUST_TEST_THREADS=3 bash "$ROOT/scripts/run-rust-integration-tests.sh"
expect_exit 2 env ATTUNE_E2E_PROJECT_NAME= bash "$ROOT/scripts/run-integration-tests.sh" --no-startup --no-build
expect_exit 2 env ATTUNE_E2E_PROJECT_NAME= bash "$ROOT/scripts/run-rust-integration-tests.sh" --no-startup --no-build
expect_exit 2 env ATTUNE_E2E_RUN_ID=guard-attach ATTUNE_E2E_PROJECT_NAME=attune-guard-attach \
  bash "$ROOT/scripts/run-integration-tests.sh" --no-startup
expect_exit 2 env ATTUNE_E2E_RUN_ID=guard-attach ATTUNE_E2E_PROJECT_NAME=attune-guard-attach \
  bash "$ROOT/scripts/run-rust-integration-tests.sh" --no-startup

grep -Fq 'test "$(cat /proc/1/comm)" = postgres' \
  "$ROOT/scripts/run-rust-integration-tests.sh" || {
    echo "Rust integration runner must reject PostgreSQL's temporary initialization server" >&2
    exit 1
  }

mkdir "$TMP_ROOT/bin"
cat > "$TMP_ROOT/bin/psql" <<'EOF'
#!/usr/bin/env sh
exit 7
EOF
chmod +x "$TMP_ROOT/bin/psql"
expect_exit 7 env PATH="$TMP_ROOT/bin:$PATH" ATTUNE_TEST_RUN_ID=guard1 \
  DATABASE_URL=postgresql://example.invalid/attune_test \
  bash "$ROOT/scripts/cleanup-test-schemas.sh" --force

mkdir "$TMP_ROOT/ci-bin"
cat > "$TMP_ROOT/ci-bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$CAPTURE_DIR/cargo-args"
printf '%s\n' "${ATTUNE__ENVIRONMENT:-}" > "$CAPTURE_DIR/environment"
printf '%s\n' "${DATABASE_URL:-}" > "$CAPTURE_DIR/database-url"
printf '%s\n' "${ATTUNE__DATABASE__URL:-}" > "$CAPTURE_DIR/attune-database-url"
EOF
chmod +x "$TMP_ROOT/ci-bin/cargo"
CAPTURE_DIR="$TMP_ROOT" PATH="$TMP_ROOT/ci-bin:$PATH" ATTUNE_RUST_TEST_THREADS=4 \
  DATABASE_URL=postgresql://example.invalid/attune_test \
  bash "$ROOT/scripts/run-ci-rust-tests.sh"
grep -Fq -- '--workspace --all-features -- --test-threads=4' "$TMP_ROOT/cargo-args"
grep -Fxq 'test' "$TMP_ROOT/environment"
grep -Fxq 'postgresql://example.invalid/attune_test' "$TMP_ROOT/database-url"
grep -Fxq 'postgresql://example.invalid/attune_test' "$TMP_ROOT/attune-database-url"
expect_exit 2 env ATTUNE_RUST_TEST_THREADS=3 \
  DATABASE_URL=postgresql://example.invalid/attune_test \
  bash "$ROOT/scripts/run-ci-rust-tests.sh"

grep -Fq 'ATTUNE_RUST_TEST_THREADS: 4' "$ROOT/.github/workflows/ci.yml"
grep -Fq 'run: scripts/run-ci-rust-tests.sh' "$ROOT/.github/workflows/ci.yml"
grep -Fq 'TEST_THREADS: ${ATTUNE_RUST_TEST_THREADS:-4}' "$ROOT/docker-compose.e2e.yaml"
grep -Fq 'cargo test --workspace --all-features --no-run' "$ROOT/docker/Dockerfile.rust-tests"

# Infrastructure-only changes must exercise the Rust runner and full-stack lane.
for path in Makefile scripts/run-ci-rust-tests.sh scripts/runtime-log-minio.sh \
  config.test.yaml docker-compose.yaml docker-compose.e2e.yaml \
  docker/Dockerfile.rust-tests .github/workflows/ci.yml; do
  flags=$(printf '%s\n' "$path" | bash "$ROOT/scripts/ci-changed-paths.sh")
  grep -Fxq 'rust=true' <<< "$flags"
  grep -Fxq 'smoke=true' <<< "$flags"
  grep -Fxq 'release=true' <<< "$flags"
done
flags=$(printf '%s\n' crates/common/src/lib.rs | bash "$ROOT/scripts/ci-changed-paths.sh")
grep -Fxq 'rust=true' <<< "$flags"
flags=$(printf '%s\n' tests/e2e/api/test_sse_execution_stream.py | bash "$ROOT/scripts/ci-changed-paths.sh")
grep -Fxq 'smoke=true' <<< "$flags"
flags=$(printf '%s\n' web/src/App.tsx | bash "$ROOT/scripts/ci-changed-paths.sh")
grep -Fxq 'web=true' <<< "$flags"
grep -Fxq 'rust=false' <<< "$flags"
flags=$(printf '%s\n' docs/testing/running-tests.md | bash "$ROOT/scripts/ci-changed-paths.sh")
if grep -q '=true' <<< "$flags"; then
  echo "documentation-only changes must not start test infrastructure" >&2
  exit 1
fi

# Exercise the Make recipe without provisioning storage or running Cargo.
cat > "$TMP_ROOT/ci-bin/docker" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$CAPTURE_DIR/docker-args"
EOF
chmod +x "$TMP_ROOT/ci-bin/docker"
: > "$TMP_ROOT/cargo-args"
CAPTURE_DIR="$TMP_ROOT" PATH="$TMP_ROOT/ci-bin:$PATH" \
  make --no-print-directory -s -C "$ROOT" test-runtime-log-correctness
[[ $(wc -l < "$TMP_ROOT/cargo-args") -eq 2 ]]
grep -Fq -- '-p attune-common --lib blob_store::tests::s3_direct_upload_authorization_puts_and_verifies_exact_bytes -- --ignored --exact --test-threads=4' "$TMP_ROOT/cargo-args"
grep -Fq -- '-p attune-api --test runtime_log_replica_tests -- --ignored --test-threads=4 --skip bounded_runtime_log_load_report --skip volume_transport_child_holds_lock' "$TMP_ROOT/cargo-args"
grep -Fq -- 'mc rm --recursive --force --versions' "$TMP_ROOT/docker-args"

echo "test isolation guard checks passed"
