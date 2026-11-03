#!/usr/bin/env bash
set -Eeuo pipefail

usage() {
    printf 'Usage: %s <asset-directory> <version> <tag> [--allow-missing-cli] [--allow-missing-linux-packages]\n' "$0"
    printf 'Example: %s release-assets 1.2.3 v1.2.3 --allow-missing-cli --allow-missing-linux-packages\n' "$0"
}
if [ "$#" -eq 1 ] && [ "$1" = --help ]; then
    usage
    exit 0
fi
if [ "$#" -lt 3 ]; then
    usage >&2
    exit 2
fi

asset_dir=$1
version=$2
tag=$3
shift 3
allow_missing_cli=false
allow_missing_linux_packages=false
for option in "$@"; do
    case "$option" in
        --allow-missing-cli) allow_missing_cli=true ;;
        --allow-missing-linux-packages) allow_missing_linux_packages=true ;;
        *) printf 'Unknown option: %s\n' "$option" >&2; usage >&2; exit 2 ;;
    esac
done

require_file() {
    if [ ! -f "$asset_dir/$1" ]; then
        printf 'Missing release asset: %s\n' "$1" >&2
        exit 1
    fi
}

require_cli_archive() {
    if [ "$allow_missing_cli" = true ] && [ ! -e "$asset_dir/$1" ] && [ ! -e "$asset_dir/$1.sha256" ]; then
        printf 'Optional CLI archive unavailable: %s\n' "$1"
        return
    fi
    require_file "$1"
    require_file "$1.sha256"
}

for arch in amd64 arm64; do
    require_file "attune-binaries-${arch}.tar.gz"
    require_cli_archive "attune_${version}_linux_${arch}.tar.gz"
    require_cli_archive "attune_${version}_darwin_${arch}.tar.gz"
done

require_cli_archive "attune_${version}_windows_amd64.zip"
require_file "attune-docker-dist-${tag}.tar.gz"
require_file "attune-arch-package-keyring.asc"
require_file "attune-openapi.json"

shopt -s nullglob
package_files=("$asset_dir"/*.deb "$asset_dir"/*.rpm "$asset_dir"/*.pkg.tar.zst "$asset_dir"/*.pkg.tar.zst.sig)
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
if [ "$allow_missing_linux_packages" = true ] && [ "${#package_files[@]}" -eq 0 ]; then
    printf 'Optional Linux package set unavailable\n'
    package_names=()
fi
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
