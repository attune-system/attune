#!/bin/sh
# Acceptance toolchain only: guard every Cargo operation inside the owned build.
set -eu
probe=${CARGO_TARGET_DIR:-/}
while [ ! -e "$probe" ]; do probe=$(dirname "$probe"); done
available=$(df -PB1 "$probe" | awk 'NR == 2 {print $4}')
printf 'Cargo disk gate: target=%s available_bytes=%s minimum=12884901888\n' "$probe" "$available"
case "$available" in ''|*[!0-9]*) exit 75;; esac
[ "$available" -ge 12884901888 ] || exit 75
exec /usr/local/cargo/bin/cargo "$@"
