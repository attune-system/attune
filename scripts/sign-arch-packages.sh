#!/usr/bin/env bash
set -Eeuo pipefail

if [ "$#" -ne 2 ]; then
    printf 'Usage: %s <package-directory> <public-key>\n' "$0" >&2
    exit 2
fi

: "${ARCH_PACKAGE_SIGNING_KEY_BASE64:?ARCH_PACKAGE_SIGNING_KEY_BASE64 is required}"
: "${ARCH_PACKAGE_SIGNING_KEY_PASSPHRASE:?ARCH_PACKAGE_SIGNING_KEY_PASSPHRASE is required}"
: "${SOURCE_DATE_EPOCH:?SOURCE_DATE_EPOCH is required}"

package_dir=$1
public_key=$2
signing_key_base64=$ARCH_PACKAGE_SIGNING_KEY_BASE64
signing_key_passphrase=$ARCH_PACKAGE_SIGNING_KEY_PASSPHRASE
unset ARCH_PACKAGE_SIGNING_KEY_BASE64 ARCH_PACKAGE_SIGNING_KEY_PASSPHRASE
signing_home=$(mktemp -d)
verification_home=$(mktemp -d)
private_key=$(mktemp)
trap 'rm -rf "$signing_home" "$verification_home" "$private_key"' EXIT
chmod 700 "$signing_home" "$verification_home"

printf '%s' "$signing_key_base64" | base64 --decode > "$private_key"
unset signing_key_base64
gpg --batch --homedir "$signing_home" --import "$private_key" >/dev/null 2>&1
gpg --batch --homedir "$verification_home" --import "$public_key" >/dev/null 2>&1

mapfile -t secret_fingerprints < <(
    gpg --batch --homedir "$signing_home" --with-colons --list-secret-keys |
        awk -F: '$1 == "sec" { primary = 1; next } primary && $1 == "fpr" { print $10; primary = 0 }'
)
mapfile -t public_fingerprints < <(
    gpg --batch --homedir "$verification_home" --with-colons --list-keys |
        awk -F: '$1 == "pub" { primary = 1; next } primary && $1 == "fpr" { print $10; primary = 0 }'
)

if [ "${#secret_fingerprints[@]}" -ne 1 ]; then
    printf 'Expected exactly one private signing key, found %d\n' "${#secret_fingerprints[@]}" >&2
    exit 1
fi
if [ "${#public_fingerprints[@]}" -ne 1 ]; then
    printf 'Expected exactly one public signing key, found %d\n' "${#public_fingerprints[@]}" >&2
    exit 1
fi
if [ "${secret_fingerprints[0]}" != "${public_fingerprints[0]}" ]; then
    printf 'Private signing key does not match %s\n' "$public_key" >&2
    exit 1
fi

shopt -s nullglob
packages=("$package_dir"/*.pkg.tar.zst)
if [ "${#packages[@]}" -eq 0 ]; then
    printf 'No Arch packages found in %s\n' "$package_dir" >&2
    exit 1
fi

for package_file in "${packages[@]}"; do
    signature_file="${package_file}.sig"
    printf '%s' "$signing_key_passphrase" | gpg --batch --yes --homedir "$signing_home" \
        --pinentry-mode loopback \
        --passphrase-fd 0 \
        --local-user "${secret_fingerprints[0]}" \
        --faked-system-time "$SOURCE_DATE_EPOCH" \
        --output "$signature_file" \
        --detach-sign "$package_file"
    gpg --batch --homedir "$verification_home" \
        --verify "$signature_file" "$package_file" >/dev/null 2>&1
    printf 'Signed %s\n' "$(basename "$package_file")"
done
