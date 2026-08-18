#!/bin/sh
set -eu

package_dir=${1:?package directory is required}
version=${2:?package version is required}
deb_arch=${3:-amd64}
rpm_arch=${4:-x86_64}
pacman_arch=${5:-x86_64}
packages="attune-agent attune-api attune-cli attune-common attune-executor attune-notifier attune-supervisor attune"
services="attune-api attune-executor attune-notifier attune-supervisor attune"

deb_control() {
    ar p "$1" control.tar.gz | tar -xzO ./control
}

arch_metadata() {
    tar --zstd -xOf "$1" .PKGINFO
}

arch_install() {
    tar --zstd -xOf "$1" .INSTALL
}

for package in $packages; do
    test -f "$package_dir/${package}_${version}_${deb_arch}.deb"
    test -f "$package_dir/${package}-${version}-1.${rpm_arch}.rpm"
    test -f "$package_dir/${package}-${version}-1-${pacman_arch}.pkg.tar.zst"
done

for service in $services; do
    deb_file="$package_dir/${service}_${version}_${deb_arch}.deb"
    control=$(deb_control "$deb_file")
    printf '%s\n' "$control" | grep -Fq "Pre-Depends: attune-common (= $version)"
    printf '%s\n' "$control" | grep -Fq "Depends: attune-common (= $version)"

    arch_file="$package_dir/${service}-${version}-1-${pacman_arch}.pkg.tar.zst"
    metadata=$(arch_metadata "$arch_file")
    install_script=$(arch_install "$arch_file")
    printf '%s\n' "$metadata" | grep -Fq "depend = attune-common=$version"
    printf '%s\n' "$install_script" | grep -Fq 'function pre_upgrade()'
    printf '%s\n' "$install_script" | grep -Fq 'function post_upgrade()'

    rpm_file="$package_dir/${service}-${version}-1.${rpm_arch}.rpm"
    if command -v rpm >/dev/null 2>&1; then
        rpm -qp --requires "$rpm_file" | grep -Fq "attune-common = $version"
        rpm -qp --scripts "$rpm_file" | grep -Fq 'posttrans scriptlet'
    else
        strings "$rpm_file" | grep -Fq 'attune-common'
        strings "$rpm_file" | grep -Fq 'postinstall_service'
    fi
done

common_deb="$package_dir/attune-common_${version}_${deb_arch}.deb"
common_control=$(deb_control "$common_deb")
printf '%s\n' "$common_control" | grep -Fq 'Depends: python3'
ar p "$common_deb" data.tar.gz | tar -tz | grep -Fq './etc/attune/attune.yaml'
ar p "$common_deb" data.tar.gz | tar -tz | grep -Fq './usr/lib/attune/package-hooks/all-in-one-links.sh'

all_deb="$package_dir/attune_${version}_${deb_arch}.deb"
if ar p "$all_deb" data.tar.gz | tar -tz | grep -q '^\./var/lib/attune'; then
    echo 'all-in-one Debian package owns attune-common state directories' >&2
    exit 1
fi
ar p "$all_deb" control.tar.gz | tar -xzO ./postinst | grep -Fq 'all-in-one-links.sh'

all_arch="$package_dir/attune-${version}-1-${pacman_arch}.pkg.tar.zst"
if tar --zstd -tf "$all_arch" | grep -q '^var/lib/attune'; then
    echo 'all-in-one Arch package owns attune-common state directories' >&2
    exit 1
fi
arch_install "$all_arch" | grep -Fq 'all-in-one-links.sh'
strings "$package_dir/attune-${version}-1.${rpm_arch}.rpm" | grep -Fq 'all-in-one-links.sh'
tar --zstd -tf "$package_dir/attune-common-${version}-1-${pacman_arch}.pkg.tar.zst" \
    | grep -Fq 'usr/lib/attune/package-hooks/all-in-one-links.sh'

echo "nFPM package metadata tests passed"
