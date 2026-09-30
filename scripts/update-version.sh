#!/usr/bin/env bash
# Update workspace package metadata after changing the platform version.
#
# Usage:
#   1. Change [workspace.package].version in Cargo.toml.
#   2. Run ./scripts/update-version.sh

set -euo pipefail

if [ "$#" -ne 0 ]; then
    echo "Do not pass a version here; change [workspace.package].version in Cargo.toml." >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$PROJECT_ROOT"

VERSION="$(python3 - <<'PY'
import re
from pathlib import Path

content = Path("Cargo.toml").read_text(encoding="utf-8")
match = re.search(
    r"(?ms)^\[workspace\.package\]\n.*?^version\s*=\s*\"([^\"]+)\"",
    content,
)
if match is None:
    raise SystemExit("Could not find [workspace.package].version in Cargo.toml")

version = match.group(1)
if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
    raise SystemExit(f"Workspace version must be stable semantic version, got {version!r}")
print(version)
PY
)"

# The timer sensor is versioned independently. Do not update its manifest or
# lock file during a platform release.
cargo update --workspace

cargo metadata --locked --no-deps --format-version 1 >/dev/null
cargo metadata \
    --manifest-path crates/core-timer-sensor/Cargo.toml \
    --locked \
    --no-deps \
    --format-version 1 >/dev/null

echo "Updated platform release version to ${VERSION}."
