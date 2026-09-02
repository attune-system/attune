#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
test_root=$(mktemp -d)
signing_home="$test_root/signing-home"
package_dir="$test_root/packages"
passphrase='test-signing-passphrase'
trap 'rm -rf "$test_root"' EXIT
mkdir -m 700 "$signing_home"
mkdir "$package_dir"

gpg --batch --homedir "$signing_home" --pinentry-mode loopback \
    --passphrase "$passphrase" \
    --quick-generate-key 'Attune package test <test@attune.local>' rsa2048 sign 1d \
    >/dev/null 2>&1
gpg --batch --homedir "$signing_home" --armor \
    --export 'Attune package test' > "$test_root/public.asc"
ARCH_PACKAGE_SIGNING_KEY_BASE64=$(
    gpg --batch --homedir "$signing_home" --pinentry-mode loopback \
        --passphrase "$passphrase" --export-secret-keys |
        base64 --wrap=0
)
export ARCH_PACKAGE_SIGNING_KEY_BASE64
export ARCH_PACKAGE_SIGNING_KEY_PASSPHRASE=$passphrase
SOURCE_DATE_EPOCH=$(date +%s)
export SOURCE_DATE_EPOCH

printf 'test package contents\n' > "$package_dir/attune-cli-1.2.3-1-x86_64.pkg.tar.zst"
if bash "$repo_root/scripts/sign-arch-packages.sh" \
    "$package_dir" "$repo_root/packaging/keys/attune-arch-package-keyring.asc"; then
    printf 'Mismatched private signing key was accepted\n' >&2
    exit 1
fi
bash "$repo_root/scripts/sign-arch-packages.sh" "$package_dir" "$test_root/public.asc"
bash "$repo_root/scripts/verify-arch-package-signatures.sh" "$package_dir" "$test_root/public.asc"

first_digest=$(sha256sum "$package_dir/attune-cli-1.2.3-1-x86_64.pkg.tar.zst.sig")
first_digest=${first_digest%% *}
bash "$repo_root/scripts/sign-arch-packages.sh" "$package_dir" "$test_root/public.asc"
second_digest=$(sha256sum "$package_dir/attune-cli-1.2.3-1-x86_64.pkg.tar.zst.sig")
second_digest=${second_digest%% *}
if [ "$first_digest" != "$second_digest" ]; then
    printf 'Repeated signing produced a different signature\n' >&2
    exit 1
fi

printf 'tampered\n' >> "$package_dir/attune-cli-1.2.3-1-x86_64.pkg.tar.zst"
if bash "$repo_root/scripts/verify-arch-package-signatures.sh" \
    "$package_dir" "$test_root/public.asc"; then
    printf 'Tampered Arch package passed signature verification\n' >&2
    exit 1
fi

printf 'Arch package signature tests passed\n'
