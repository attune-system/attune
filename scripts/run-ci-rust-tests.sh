#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
threads="${ATTUNE_RUST_TEST_THREADS:-4}"
database_url="${DATABASE_URL:-${ATTUNE__DATABASE__URL:-}}"

"$ROOT/scripts/check-db-test-threads.sh" "$threads"
bash "$ROOT/scripts/database-test-ignore-policy.sh" --check

if [[ -z "$database_url" ]]; then
  echo "ERROR: DATABASE_URL or ATTUNE__DATABASE__URL is required for CI Rust tests" >&2
  exit 2
fi

export ATTUNE__ENVIRONMENT=test
export DATABASE_URL="$database_url"
export ATTUNE__DATABASE__URL="$database_url"

cd "$ROOT"
if [[ $# -ne 0 ]]; then
  exec python3 scripts/ci-rust-test-artifacts.py run "$@" "$threads"
fi
if [[ -n "${ATTUNE_CI_BROKER_TEST_ARTIFACTS:-}" ]]; then
  # Record executables from this build so the broker tests need no Cargo rebuilds.
  cargo test --workspace --all-features --message-format=json-render-diagnostics -- --test-threads="$threads" |
    python3 -u scripts/run-ci-completion-broker-tests.py record "$ATTUNE_CI_BROKER_TEST_ARTIFACTS"
  exit 0
fi
cargo test --workspace --all-features -- --test-threads="$threads"
