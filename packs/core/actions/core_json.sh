#!/bin/sh
# Match the executable alternatives verified by the bundled Python runtime.
# Version checks must not consume the JSON waiting on stdin.
set -eu
for interpreter in python3.13 python3.12 python3.11 python3 python; do
    if command -v "$interpreter" >/dev/null 2>&1 &&
        "$interpreter" -c 'import sys; sys.exit(not ((3, 11) <= sys.version_info[:2] < (4, 0)))' </dev/null 2>/dev/null; then
        exec "$interpreter" "${0%/*}/core_actions.py" "$@"
    fi
done
printf '%s\n' 'ERROR: core JSON actions require a runnable Python >=3.11,<4 interpreter' >&2
exit 126
