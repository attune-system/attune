#!/bin/sh
# Both transports publish through the API. Never overwrite the active pack projection.
set -eu
case "${ATTUNE_BOOTSTRAP_MODE:-volume}" in
    api|volume) ;;
    *) echo 'ATTUNE_BOOTSTRAP_MODE must be api or volume' >&2; exit 2 ;;
esac
pip install --quiet --no-cache-dir pyyaml
if [ "${ATTUNE_BOOTSTRAP_MODE:-volume}" = volume ]; then
    for directory in "${TARGET_PACKS_DIR:-/opt/attune/packs}" "${RUNTIME_ENVS_DIR:-/opt/attune/runtime_envs}" "${ARTIFACTS_DIR:-/opt/attune/artifacts}"; do
        mkdir -p "$directory"
        chown 1000:1000 "$directory"
    done
    # Binary initialization may have created root-owned projection directories.
    # Preserve the existing volume ownership contract without changing bytes.
    chown -R 1000:1000 "${TARGET_PACKS_DIR:-/opt/attune/packs}"
fi

# Volume packaging may supply compiled binaries separately. Assemble a private
# candidate, not a mutable replacement of the installed core directory.
candidate=$(mktemp -d)
trap 'rm -rf "$candidate"' EXIT HUP INT TERM
cp -R "${SOURCE_PACKS_DIR:-/source/packs}/core" "$candidate/core"
binary="${TARGET_PACKS_DIR:-/opt/attune/packs}/core/sensors/attune-core-timer-sensor"
if [ "${ATTUNE_BOOTSTRAP_MODE:-volume}" = volume ] && [ -f "$binary" ]; then
    cp "$binary" "$candidate/core/sensors/attune-core-timer-sensor"
fi
SOURCE_PACKS_DIR="$candidate" python3 /scripts/bootstrap_core_pack.py publish
