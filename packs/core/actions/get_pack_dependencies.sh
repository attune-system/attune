#!/bin/sh
set -eu
exec python3 "$(dirname "$0")/pack_api.py" get_pack_dependencies
