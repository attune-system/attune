#!/usr/bin/env bash
set -euo pipefail

threads="${1:-${ATTUNE_RUST_TEST_THREADS:-4}}"

if [[ ! "$threads" =~ ^[0-9]+$ ]] || ((threads < 4)); then
  echo "ERROR: database-backed tests require at least 4 test threads, got '${threads}'" >&2
  exit 2
fi
