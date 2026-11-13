#!/bin/sh
# Canonical flat stdin JSON; keep the public shell entrypoint.
set -eu
exec python3 "$(dirname "$0")/core_actions.py" noop
