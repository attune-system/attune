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
    attune-arch-package-keyring.asc \
    attune-openapi.json; do
    touch "$test_root/$file"
done
for package_name in \
    attune attune-agent attune-api attune-cli attune-common attune-executor \
    attune-notifier attune-supervisor; do
    touch \
        "$test_root/${package_name}_${version}_amd64.deb" \
        "$test_root/${package_name}_${version}_arm64.deb" \
        "$test_root/${package_name}-${version}-1.x86_64.rpm" \
        "$test_root/${package_name}-${version}-1.aarch64.rpm" \
        "$test_root/${package_name}-${version}-1-x86_64.pkg.tar.zst" \
        "$test_root/${package_name}-${version}-1-x86_64.pkg.tar.zst.sig" \
        "$test_root/${package_name}-${version}-1-aarch64.pkg.tar.zst" \
        "$test_root/${package_name}-${version}-1-aarch64.pkg.tar.zst.sig"
done

bash "$repo_root/scripts/verify-release-assets.sh" "$test_root" "$version" "$tag"
rm "$test_root/attune-docker-dist-${tag}.tar.gz"
if bash "$repo_root/scripts/verify-release-assets.sh" "$test_root" "$version" "$tag"; then
    echo 'Incomplete release asset set was accepted' >&2
    exit 1
fi

touch "$test_root/attune-docker-dist-${tag}.tar.gz"
rm "$test_root/attune-api_${version}_arm64.deb"
if bash "$repo_root/scripts/verify-release-assets.sh" "$test_root" "$version" "$tag"; then
    echo 'Incomplete Linux package set was accepted' >&2
    exit 1
fi

touch "$test_root/attune-api_${version}_arm64.deb"
rm "$test_root/attune-${version}-1-x86_64.pkg.tar.zst.sig"
if bash "$repo_root/scripts/verify-release-assets.sh" "$test_root" "$version" "$tag"; then
    echo 'Unsigned Arch package was accepted' >&2
    exit 1
fi

touch "$test_root/attune-${version}-1-x86_64.pkg.tar.zst.sig"
touch "$test_root/orphan-${version}-x86_64.pkg.tar.zst.sig"
if bash "$repo_root/scripts/verify-release-assets.sh" "$test_root" "$version" "$tag"; then
    echo 'Orphaned Arch package signature was accepted' >&2
    exit 1
fi
