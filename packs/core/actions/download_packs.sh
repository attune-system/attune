#!/bin/sh
# Keep the public entrypoint; structured parameters and results use real JSON.
set -eu
exec python3 "$(dirname "$0")/pack_api.py" download_packs
