#!/usr/bin/env bash
set -Eeuo pipefail

if [ "$#" -ne 3 ]; then
    printf 'Usage: %s <asset-directory> <version> <tag>\n' "$0" >&2
    exit 2
fi

asset_dir=$1
version=$2
tag=$3

require_file() {
    if [ ! -f "$asset_dir/$1" ]; then
        printf 'Missing release asset: %s\n' "$1" >&2
        exit 1
    fi
}

for arch in amd64 arm64; do
    require_file "attune-binaries-${arch}.tar.gz"
    require_file "attune_${version}_linux_${arch}.tar.gz"
    require_file "attune_${version}_linux_${arch}.tar.gz.sha256"
    require_file "attune_${version}_darwin_${arch}.tar.gz"
    require_file "attune_${version}_darwin_${arch}.tar.gz.sha256"
done

require_file "attune_${version}_windows_amd64.zip"
require_file "attune_${version}_windows_amd64.zip.sha256"
require_file "attune-docker-dist-${tag}.tar.gz"
require_file "attune-arch-package-keyring.asc"
require_file "attune-openapi.json"

package_names=(
    attune
    attune-agent
    attune-api
    attune-cli
    attune-common
    attune-executor
    attune-notifier
    attune-supervisor
)
for package_name in "${package_names[@]}"; do
    require_file "${package_name}_${version}_amd64.deb"
    require_file "${package_name}_${version}_arm64.deb"
    require_file "${package_name}-${version}-1.x86_64.rpm"
    require_file "${package_name}-${version}-1.aarch64.rpm"
    require_file "${package_name}-${version}-1-x86_64.pkg.tar.zst"
    require_file "${package_name}-${version}-1-aarch64.pkg.tar.zst"
done

shopt -s nullglob
arch_packages=("$asset_dir"/*.pkg.tar.zst)
arch_signatures=("$asset_dir"/*.pkg.tar.zst.sig)
for package_file in "${arch_packages[@]}"; do
    require_file "$(basename "$package_file").sig"
done
for signature_file in "${arch_signatures[@]}"; do
    require_file "$(basename "${signature_file%.sig}")"
done

printf 'Verified release asset families in %s\n' "$asset_dir"
