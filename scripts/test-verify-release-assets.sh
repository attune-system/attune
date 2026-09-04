#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT

version=1.2.3
tag=v1.2.3
for file in \
    attune-binaries-amd64.tar.gz \
    attune-binaries-arm64.tar.gz \
    "attune_${version}_linux_amd64.tar.gz" \
    "attune_${version}_linux_amd64.tar.gz.sha256" \
    "attune_${version}_linux_arm64.tar.gz" \
    "attune_${version}_linux_arm64.tar.gz.sha256" \
    "attune_${version}_darwin_amd64.tar.gz" \
    "attune_${version}_darwin_amd64.tar.gz.sha256" \
    "attune_${version}_darwin_arm64.tar.gz" \
    "attune_${version}_darwin_arm64.tar.gz.sha256" \
    "attune_${version}_windows_amd64.zip" \
    "attune_${version}_windows_amd64.zip.sha256" \
    "attune-docker-dist-${tag}.tar.gz" \
    "attune-${version}.tgz" \
    attune-arch-package-keyring.asc \
    attune-openapi.json \
    attune_amd64.deb attune_arm64.deb \
    attune_x86_64.rpm attune_aarch64.rpm \
    attune_x86_64.pkg.tar.zst attune_x86_64.pkg.tar.zst.sig \
    attune_aarch64.pkg.tar.zst attune_aarch64.pkg.tar.zst.sig; do
    touch "$test_root/$file"
done

bash "$repo_root/scripts/verify-release-assets.sh" "$test_root" "$version" "$tag"
rm "$test_root/attune-${version}.tgz"
if bash "$repo_root/scripts/verify-release-assets.sh" "$test_root" "$version" "$tag"; then
    echo 'Incomplete release asset set was accepted' >&2
    exit 1
fi

touch "$test_root/attune-${version}.tgz"
rm "$test_root/attune_x86_64.pkg.tar.zst.sig"
if bash "$repo_root/scripts/verify-release-assets.sh" "$test_root" "$version" "$tag"; then
    echo 'Unsigned Arch package was accepted' >&2
    exit 1
fi

touch "$test_root/attune_x86_64.pkg.tar.zst.sig"
touch "$test_root/orphan_x86_64.pkg.tar.zst.sig"
if bash "$repo_root/scripts/verify-release-assets.sh" "$test_root" "$version" "$tag"; then
    echo 'Orphaned Arch package signature was accepted' >&2
    exit 1
fi
