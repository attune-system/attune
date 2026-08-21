#!/usr/bin/env bash

# Uninstall every currently installed pack from the configured standard index,
# then reinstall the exact refs and versions returned by that index.
set -Eeuo pipefail

attune_bin="${ATTUNE_BIN:-attune}"
output_dir="${ATTUNE_REINSTALL_LOG_DIR:-${TMPDIR:-/tmp}}"
dry_run=false
case "${1:-}" in
    "") ;;
    --dry-run) dry_run=true ;;
    *) printf 'Usage: %s [--dry-run]\n' "$0" >&2; exit 2 ;;
esac
index_file="$(mktemp "${output_dir%/}/attune-standard-index.XXXXXX.json")"
installed_file="$(mktemp "${output_dir%/}/attune-installed-packs.XXXXXX.json")"
trap 'rm -f "$index_file" "$installed_file"' EXIT

if ! command -v "$attune_bin" >/dev/null 2>&1; then
    printf 'Attune CLI not found: %s\n' "$attune_bin" >&2
    exit 127
fi
if ! command -v jq >/dev/null 2>&1; then
    printf 'jq is required to parse registry and pack output\n' >&2
    exit 127
fi

printf 'Reading the standard pack index...\n'
"$attune_bin" pack index browse --output json >"$index_file"

registry_id="$(jq -er '.[0].registry.id' "$index_file")"
pack_count="$(jq -er 'length' "$index_file")"
if [[ "$pack_count" -eq 0 ]]; then
    printf 'The standard pack index is empty; refusing to uninstall anything\n' >&2
    exit 1
fi
if ! jq -e --argjson expected_registry_id "$registry_id" \
    'all(.[]; .registry.id == $expected_registry_id and (.pack.ref | type) == "string" and (.pack.version | type) == "string")' \
    "$index_file" >/dev/null; then
    printf 'The index contains inconsistent registry metadata\n' >&2
    exit 1
fi

"$attune_bin" pack list --output json >"$installed_file"
installed_refs="$(jq -r '.[].ref' "$installed_file")"

printf 'Standard index: %s packs (registry id %s)\n' "$pack_count" "$registry_id"
if "$dry_run"; then
    printf 'Dry run; no packs changed. Planned refs:\n'
    jq -r '.[] | "  \(.pack.ref)@\(.pack.version)"' "$index_file"
    exit 0
fi

printf 'Uninstalling indexed packs that are currently installed...\n'
while IFS=$'\t' read -r pack_ref pack_version; do
    if grep -Fxq "$pack_ref" <<<"$installed_refs"; then
        printf '  uninstall %s@%s\n' "$pack_ref" "$pack_version"
        "$attune_bin" pack uninstall "$pack_ref" --yes --output json >/dev/null
    else
        printf '  skip %s@%s (not installed)\n' "$pack_ref" "$pack_version"
    fi
done < <(jq -r '.[] | [.pack.ref, .pack.version] | @tsv' "$index_file")

printf 'Reinstalling indexed packs with tests enabled...\n'
while IFS=$'\t' read -r pack_ref pack_version; do
    pack_spec="${pack_ref}@${pack_version}"
    printf '  install %s\n' "$pack_spec"
    install_response="$(
        "$attune_bin" pack install "$pack_spec" \
            --registry-id "$registry_id" \
            --output json
    )"
    install_status="$(jq -r '.install_status // empty' <<<"$install_response")"
    case "$install_status" in
        succeeded)
            ;;
        "")
            # Packs without a testing block legitimately have no install status.
            # Since this script starts from an uninstall, verify the installed
            # ref/version instead.
            if ! "$attune_bin" pack list --output json \
                | jq -e --arg pack_ref "$pack_ref" --arg pack_version "$pack_version" \
                    'any(.[]; .ref == $pack_ref and .version == $pack_version)' >/dev/null; then
                printf 'Installation did not succeed for %s:\n%s\n' "$pack_spec" "$install_response" >&2
                exit 1
            fi
            ;;
        *)
            printf 'Installation did not succeed for %s (status: %s):\n%s\n' \
                "$pack_spec" "$install_status" "$install_response" >&2
            exit 1
            ;;
    esac
done < <(jq -r '.[] | [.pack.ref, .pack.version] | @tsv' "$index_file")

printf 'Successfully reinstalled %s standard-index packs.\n' "$pack_count"
