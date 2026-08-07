#!/usr/bin/env bash
set -euo pipefail

if command -v semgrep >/dev/null 2>&1; then
    semgrep_command=(semgrep)
elif command -v uvx >/dev/null 2>&1; then
    semgrep_command=(uvx semgrep)
else
    echo "Semgrep is unavailable; install semgrep or uv." >&2
    exit 1
fi

exec "${semgrep_command[@]}" scan \
    --config p/default \
    --error \
    --timeout 120 \
    --timeout-threshold 0 \
    "$@"
