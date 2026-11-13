#!/bin/sh
set -eu
exec "$(dirname "$0")/../test_core_pack.sh" "$@"
