#!/usr/bin/env bash
set -Eeuo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
    printf 'Usage: %s <tag> <file> [asset-name]\n' "$0" >&2
    exit 2
fi

tag=$1
file=$2
asset_name=${3:-$(basename "$file")}

if [ ! -f "$file" ]; then
    printf 'Release asset does not exist: %s\n' "$file" >&2
    exit 1
fi

release_json=$(gh release view "$tag" --json isDraft,assets)
if jq -e --arg name "$asset_name" 'any(.assets[]?; .name == $name)' \
    <<<"$release_json" >/dev/null; then
    download_dir=$(mktemp -d)
    trap 'rm -rf "$download_dir"' EXIT
    gh release download "$tag" --pattern "$asset_name" --dir "$download_dir"
    if ! cmp -s "$file" "$download_dir/$asset_name"; then
        printf 'Release asset %s already exists with different content\n' "$asset_name" >&2
        exit 1
    fi
    printf 'Release asset %s already exists with identical content\n' "$asset_name"
    exit 0
fi

if [ "$(jq -r '.isDraft' <<<"$release_json")" != true ]; then
    printf 'Release %s is public and asset %s is missing; refusing to mutate it\n' \
        "$tag" "$asset_name" >&2
    exit 1
fi

upload_file=$file
if [ "$(basename "$file")" != "$asset_name" ]; then
    upload_dir=$(mktemp -d)
    trap 'rm -rf "${download_dir:-}" "${upload_dir:-}"' EXIT
    cp -- "$file" "$upload_dir/$asset_name"
    upload_file="$upload_dir/$asset_name"
fi

gh release upload "$tag" "$upload_file"
