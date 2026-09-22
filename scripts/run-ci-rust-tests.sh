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
cargo test --workspace --all-features -- --test-threads="$threads"
