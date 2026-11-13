#!/bin/sh
# Both transports publish through the API. Never overwrite the active pack projection.
set -eu
export ATTUNE_BOOTSTRAP_MODE="${ATTUNE_BOOTSTRAP_MODE:-volume}"
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
binary_root="${PACK_BINARIES_DIR:-/opt/attune/pack-binaries}/core"
binary="$binary_root/sensors/attune-core-timer-sensor"
if [ "${ATTUNE_BOOTSTRAP_MODE:-volume}" = volume ] && [ -f "$binary" ]; then
    cp "$binary" "$candidate/core/sensors/attune-core-timer-sensor"
fi
# Local Compose assembles exactly its host variant. Never overlay the active
# projection's removed metadata or carry foreign/stale variants into a candidate.
if [ "${ATTUNE_BOOTSTRAP_MODE:-volume}" = volume ]; then
    case "$(uname -m)" in
        x86_64|amd64) target=linux-amd64 ;;
        aarch64|arm64) target=linux-arm64 ;;
        *) echo 'Unsupported local core action architecture' >&2; exit 1 ;;
    esac
    for action in echo sleep noop; do
        binary="$binary_root/actions/bin/$action/$target"
        test -x "$binary" || { echo 'Missing local native core action' >&2; exit 1; }
        rm -rf "$candidate/core/actions/bin/$action"
        mkdir -p "$candidate/core/actions/bin/$action"
        cp "$binary" "$candidate/core/actions/bin/$action/$target"
    done
fi
SOURCE_PACKS_DIR="$candidate" python3 /scripts/bootstrap_core_pack.py publish
