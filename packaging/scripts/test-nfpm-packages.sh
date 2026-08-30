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
for dependency in python3 wget postgresql-client coreutils; do
    printf '%s\n' "$common_control" | grep -F 'Depends:' | grep -Fq "$dependency"
done
ar p "$common_deb" data.tar.gz | tar -tz | grep -Fq './etc/attune/attune.yaml'
ar p "$common_deb" data.tar.gz | tar -tz | grep -Fq './usr/lib/attune/package-hooks/all-in-one-links.sh'

common_arch="$package_dir/attune-common-${version}-1-${pacman_arch}.pkg.tar.zst"
common_arch_metadata=$(arch_metadata "$common_arch")
for dependency in python wget postgresql-libs coreutils; do
    printf '%s\n' "$common_arch_metadata" | grep -Fq "depend = $dependency"
done

common_rpm="$package_dir/attune-common-${version}-1.${rpm_arch}.rpm"
if command -v rpm >/dev/null 2>&1; then
    common_rpm_requires=$(rpm -qp --requires "$common_rpm")
    for dependency in python3 wget postgresql coreutils; do
        printf '%s\n' "$common_rpm_requires" | grep -Fq "$dependency"
    done
else
    for dependency in python3 wget postgresql coreutils; do
        strings "$common_rpm" | grep -Fq "$dependency"
    done
fi

all_deb="$package_dir/attune_${version}_${deb_arch}.deb"
if ar p "$all_deb" data.tar.gz | tar -tz | grep -q '^\./var/lib/attune'; then
    echo 'all-in-one Debian package owns attune-common state directories' >&2
    exit 1
fi
ar p "$all_deb" control.tar.gz | tar -xzO ./postinst | grep -Fq 'all-in-one-links.sh'
all_deb_contents=$(ar p "$all_deb" data.tar.gz | tar -tz)
printf '%s\n' "$all_deb_contents" | grep -Fq './usr/share/bash-completion/completions/attune'
printf '%s\n' "$all_deb_contents" | grep -Fq './usr/share/fish/vendor_completions.d/attune.fish'
printf '%s\n' "$all_deb_contents" | grep -Fq './usr/share/zsh/vendor-completions/_attune'

all_arch="$package_dir/attune-${version}-1-${pacman_arch}.pkg.tar.zst"
if tar --zstd -tf "$all_arch" | grep -q '^var/lib/attune'; then
    echo 'all-in-one Arch package owns attune-common state directories' >&2
    exit 1
fi
arch_install "$all_arch" | grep -Fq 'all-in-one-links.sh'
all_arch_contents=$(tar --zstd -tf "$all_arch")
printf '%s\n' "$all_arch_contents" | grep -Fq 'usr/share/bash-completion/completions/attune'
printf '%s\n' "$all_arch_contents" | grep -Fq 'usr/share/fish/vendor_completions.d/attune.fish'
printf '%s\n' "$all_arch_contents" | grep -Fq 'usr/share/zsh/site-functions/_attune'
strings "$package_dir/attune-${version}-1.${rpm_arch}.rpm" | grep -Fq 'all-in-one-links.sh'
if command -v rpm >/dev/null 2>&1; then
    all_rpm_contents=$(rpm -qlp "$package_dir/attune-${version}-1.${rpm_arch}.rpm")
elif command -v bsdtar >/dev/null 2>&1; then
    all_rpm_contents=$(bsdtar -tf "$package_dir/attune-${version}-1.${rpm_arch}.rpm")
else
    echo 'rpm or bsdtar is required to inspect RPM package contents' >&2
    exit 1
fi
printf '%s\n' "$all_rpm_contents" | grep -Fq '/usr/share/bash-completion/completions/attune'
printf '%s\n' "$all_rpm_contents" | grep -Fq '/usr/share/fish/vendor_completions.d/attune.fish'
printf '%s\n' "$all_rpm_contents" | grep -Fq '/usr/share/zsh/site-functions/_attune'
tar --zstd -tf "$common_arch" | grep -Fq 'usr/lib/attune/package-hooks/all-in-one-links.sh'

echo "nFPM package metadata tests passed"
