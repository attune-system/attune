#!/bin/sh
# Image-owned candidate trees must be readable by a non-root publisher. Match
# canonical file executable intent without changing file bytes or host staging.
set -eu
pack_root=${1:?pack image tree is required}
test -d "$pack_root" && test ! -L "$pack_root"
if [ -n "$(find "$pack_root" -type l -print)" ]; then
    echo 'Pack image tree must not contain symlinks' >&2
    exit 1
fi
find "$pack_root" -type d -exec chmod 0755 {} +
find "$pack_root" -type f ! -perm /111 -exec chmod 0644 {} +
find "$pack_root" -type f -perm /111 -exec chmod 0755 {} +
