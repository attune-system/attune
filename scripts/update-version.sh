#!/usr/bin/env bash
# Synchronize generated release-version fields from Cargo.toml.
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

VERSION="$VERSION" python3 - <<'PY'
import os
import re
from pathlib import Path
from typing import Dict

version = os.environ["VERSION"]
updates: Dict[Path, str] = {}


def replace_once(path: str, pattern: str, replacement: str) -> None:
    file_path = Path(path)
    content = updates.get(file_path)
    if content is None:
        content = file_path.read_text(encoding="utf-8")

    updated, count = re.subn(pattern, replacement, content)
    if count != 1:
        raise RuntimeError(f"Expected one version field in {path}, found {count}")

    updates[file_path] = updated


replace_once(
    "crates/core-timer-sensor/Cargo.toml",
    r'(?m)^(version\s*=\s*")[^"]+(")',
    rf"\g<1>{version}\2",
)
replace_once(
    "charts/attune/Chart.yaml",
    r"(?m)^(version:\s*)[^\s#]+",
    rf"\g<1>{version}",
)
replace_once(
    "charts/attune/Chart.yaml",
    r'(?m)^(appVersion:\s*")[^"]+(")',
    rf"\g<1>{version}\2",
)
replace_once(
    "charts/attune/values.yaml",
    r'(?ms)(^global:\n.*?^\s*imageTag:\s*")[^"]+(")',
    rf"\g<1>{version}\2",
)

for path, content in updates.items():
    path.write_text(content, encoding="utf-8")
PY

# Both lock files record local package versions. Update only workspace package
# entries before the publish workflow checks metadata with --locked.
cargo update --workspace
cargo update \
    --manifest-path crates/core-timer-sensor/Cargo.toml \
    --workspace

cargo metadata --locked --no-deps --format-version 1 >/dev/null
cargo metadata \
    --manifest-path crates/core-timer-sensor/Cargo.toml \
    --locked \
    --no-deps \
    --format-version 1 >/dev/null

echo "Updated release version to ${VERSION}."
