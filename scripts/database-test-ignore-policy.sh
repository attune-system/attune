#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
mode="${1:---check}"

case "$mode" in
  --check|--fix) ;;
  *) echo "Usage: $0 [--check|--fix]" >&2; exit 2 ;;
esac

mapfile -t files < <(
  rg -l '#\[ignore(?:\s*=.*)?\]' "$ROOT/crates" --glob '*.rs' \
    --glob '!common/src/repositories/pack_test.rs'
)

pattern='^[[:space:]]*#\[ignore = "(integration test .* requires database|integration test - requires PostgreSQL|requires disposable PostgreSQL|e2e test requires PostgreSQL)"\][[:space:]]*$|^[[:space:]]*#\[ignore\][[:space:]]*// Requires database[[:space:]]*$'

if [[ "$mode" == "--fix" ]]; then
  for file in "${files[@]}"; do
    perl -ni -e 'print unless /^\s*#\[ignore = "(?:integration test .* requires database|integration test - requires PostgreSQL|requires disposable PostgreSQL|e2e test requires PostgreSQL)"\]\s*$/ || /^\s*#\[ignore\]\s*\/\/ Requires database\s*$/' "$file"
  done
  perl -ni -e 'print unless /^\s*#\[ignore\]\s*$/' \
    "$ROOT/crates/api/tests/webhook_security_tests.rs"
fi

if rg -n "$pattern" "$ROOT/crates" --glob '*.rs' \
    --glob '!common/src/repositories/pack_test.rs'; then
  echo "ERROR: database-only #[ignore] attributes remain" >&2
  exit 1
fi
