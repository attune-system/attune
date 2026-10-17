#!/bin/sh
# No direct-database registration or mutable volume activation.
set -eu
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec python3 "$script_dir/load_core_pack.py" "$@"
