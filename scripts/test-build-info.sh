#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="$(cargo metadata --manifest-path "$ROOT/Cargo.toml" --locked --no-deps --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT

# Exercise the real build script in a source tree with no Git metadata.
mkdir -p "$TEMP/source/crates/common"
rustc --edition 2021 "$ROOT/crates/common/build.rs" -o "$TEMP/build-info"
unset ATTUNE_BUILD_GIT_SHA
output="$(CARGO_MANIFEST_DIR="$TEMP/source/crates/common" "$TEMP/build-info")"
[[ "$output" == *"cargo:rustc-env=ATTUNE_BUILD_GIT_SHA=unknown"* ]]
if ATTUNE_BUILD_GIT_SHA=invalid CARGO_MANIFEST_DIR="$TEMP/source/crates/common" "$TEMP/build-info" >"$TEMP/invalid.log" 2>&1; then
    echo "Invalid build SHA was accepted" >&2
    exit 1
fi

# Reuse Cargo's normal cache and prove that changing only the SHA changes both binaries.
for sha in aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb; do
    ATTUNE_BUILD_GIT_SHA="$sha" cargo build --manifest-path "$ROOT/Cargo.toml" --locked -p attune-cli --bin attune --bin attune-mcp
    ATTUNE_BUILD_GIT_SHA=runtime-override "$TARGET/debug/attune" --json info --local >"$TEMP/cli.json"
    ATTUNE_BUILD_GIT_SHA=runtime-override "$TARGET/debug/attune-mcp" --info --local >"$TEMP/mcp.json"
    python3 - "$sha" "$TEMP/cli.json" "$TEMP/mcp.json" <<'PY'
import json
import sys
from pathlib import Path

for path in sys.argv[2:]:
    data = json.loads(Path(path).read_text())
    assert data["git_sha"] == sys.argv[1], data
    assert data["version"], data
PY
done

# Leave local binaries with the normal source revision rather than a test SHA.
cargo build --manifest-path "$ROOT/Cargo.toml" --locked -p attune-cli --bin attune --bin attune-mcp
echo "Build info checks passed: Git-less fallback, invalid SHA rejection, warm-cache changes, and immutable runtime metadata."
