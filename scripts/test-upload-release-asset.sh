#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT
mkdir -p "$test_root/bin" "$test_root/state/assets"

cat >"$test_root/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail

case "$1 $2" in
    "release view")
        draft=true
        if [ -f "$MOCK_GH_STATE/published" ]; then
            draft=false
        fi
        exists=false
        if [ -f "$MOCK_GH_STATE/assets/$MOCK_ASSET_NAME" ]; then
            exists=true
        fi
        jq -n \
            --argjson draft "$draft" \
            --argjson exists "$exists" \
            --arg name "$MOCK_ASSET_NAME" \
            '{isDraft: $draft, assets: (if $exists then [{name: $name}] else [] end)}'
        ;;
    "release download")
        pattern=$5
        destination=$7
        mkdir -p "$destination"
        cp "$MOCK_GH_STATE/assets/$pattern" "$destination/$pattern"
        ;;
    "release upload")
        source=$4
        cp "$source" "$MOCK_GH_STATE/assets/$(basename "$source")"
        ;;
    *)
        printf 'Unexpected gh invocation: %s\n' "$*" >&2
        exit 2
        ;;
esac
EOF
chmod +x "$test_root/bin/gh"

export PATH="$test_root/bin:$PATH"
export MOCK_GH_STATE="$test_root/state"
export MOCK_ASSET_NAME=renamed-asset.bin
asset="$test_root/local.bin"

printf 'original content\n' >"$asset"
bash "$repo_root/scripts/upload-release-asset.sh" v1.2.3 "$asset" "$MOCK_ASSET_NAME"
cmp -s "$asset" "$MOCK_GH_STATE/assets/$MOCK_ASSET_NAME"
bash "$repo_root/scripts/upload-release-asset.sh" v1.2.3 "$asset" "$MOCK_ASSET_NAME"

touch "$MOCK_GH_STATE/published"
bash "$repo_root/scripts/upload-release-asset.sh" v1.2.3 "$asset" "$MOCK_ASSET_NAME"

printf 'different content\n' >"$asset"
if bash "$repo_root/scripts/upload-release-asset.sh" v1.2.3 "$asset" "$MOCK_ASSET_NAME"; then
    echo 'Mismatched existing asset was accepted' >&2
    exit 1
fi

if bash "$repo_root/scripts/upload-release-asset.sh" v1.2.3 "$asset" new-asset.bin; then
    echo 'Published release mutation was accepted' >&2
    exit 1
fi
