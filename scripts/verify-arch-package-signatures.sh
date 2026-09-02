#!/usr/bin/env bash
set -Eeuo pipefail

if [ "$#" -ne 2 ]; then
    printf 'Usage: %s <package-directory> <public-key>\n' "$0" >&2
    exit 2
fi

package_dir=$1
public_key=$2
verification_home=$(mktemp -d)
trap 'rm -rf "$verification_home"' EXIT
chmod 700 "$verification_home"
gpg --batch --homedir "$verification_home" --import "$public_key" >/dev/null 2>&1

shopt -s nullglob
packages=("$package_dir"/*.pkg.tar.zst)
signatures=("$package_dir"/*.pkg.tar.zst.sig)
if [ "${#packages[@]}" -eq 0 ]; then
    printf 'No Arch packages found in %s\n' "$package_dir" >&2
    exit 1
fi

for package_file in "${packages[@]}"; do
    signature_file="${package_file}.sig"
    if [ ! -f "$signature_file" ]; then
        printf 'Missing Arch package signature: %s\n' "$(basename "$signature_file")" >&2
        exit 1
    fi
    gpg --batch --homedir "$verification_home" \
        --verify "$signature_file" "$package_file" >/dev/null 2>&1 || {
        printf 'Invalid Arch package signature: %s\n' "$(basename "$signature_file")" >&2
        exit 1
    }
done

for signature_file in "${signatures[@]}"; do
    package_file=${signature_file%.sig}
    if [ ! -f "$package_file" ]; then
        printf 'Arch package signature has no package: %s\n' "$(basename "$signature_file")" >&2
        exit 1
    fi
done

printf 'Verified %d Arch package signatures\n' "${#packages[@]}"
