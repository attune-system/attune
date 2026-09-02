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
require_file "attune-${version}.tgz"
require_file "attune-arch-package-keyring.asc"

require_arch_package() {
    local extension=$1
    local arch_pattern=$2
    local matches
    shopt -s nullglob
    matches=("$asset_dir"/*"$arch_pattern"*"$extension")
    shopt -u nullglob
    if [ "${#matches[@]}" -eq 0 ]; then
        printf 'Missing %s package for architecture pattern %s\n' "$extension" "$arch_pattern" >&2
        exit 1
    fi
}

for extension in .deb .rpm .pkg.tar.zst; do
    case "$extension" in
        .deb)
            require_arch_package "$extension" amd64
            require_arch_package "$extension" arm64
            ;;
        .rpm)
            require_arch_package "$extension" x86_64
            require_arch_package "$extension" aarch64
            ;;
        .pkg.tar.zst)
            require_arch_package "$extension" x86_64
            require_arch_package "$extension" aarch64
            ;;
    esac
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
