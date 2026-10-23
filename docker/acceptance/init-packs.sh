#!/bin/sh
# The image already contains PyYAML and the complete native-inclusive core tree.
# No runtime pip install, old flat binary adoption or mutable projection copy.
set -eu
case "${ATTUNE_BOOTSTRAP_MODE:-volume}" in
    api) ;;
    volume)
        for directory in "${TARGET_PACKS_DIR:-/opt/attune/packs}" "${RUNTIME_ENVS_DIR:-/opt/attune/runtime_envs}" "${ARTIFACTS_DIR:-/opt/attune/artifacts}"; do
            mkdir -p "$directory"
            chown 1000:1000 "$directory"
        done
        ;;
    *) echo 'ATTUNE_BOOTSTRAP_MODE must be api or volume' >&2; exit 2 ;;
esac
exec python3 /scripts/bootstrap_core_pack.py publish
