#!/usr/bin/env bash
# Update every source-controlled version required by stable vX.Y.Z releases.
#
# Usage:
#   ./scripts/update-version.sh 0.2.1
#   ./scripts/update-version.sh v0.2.1

set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <MAJOR.MINOR.PATCH | vMAJOR.MINOR.PATCH>" >&2
    exit 1
fi

VERSION="${1#v}"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Version must be a stable semantic version such as 0.2.1" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$PROJECT_ROOT"

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
    "Cargo.toml",
    r'(?ms)(^\[workspace\.package\]\n.*?^version\s*=\s*")[^"]+(")',
    rf"\g<1>{version}\2",
)
replace_once(
    "crates/core-timer-sensor/Cargo.toml",
    r'(?m)^(version\s*=\s*")[^"]+(")',
    rf"\g<1>{version}\2",
)
replace_once(
    "crates/api/src/openapi.rs",
    r'(?m)^(\s*version\s*=\s*")[^"]+(")',
    rf"\g<1>{version}\2",
)
replace_once(
    "crates/api/src/openapi.rs",
    r'(assert_eq!\(doc\.info\.version,\s*")[^"]+(")',
    rf"\g<1>{version}\2",
)
replace_once(
    "crates/api/src/routes/health.rs",
    r'(/// Service version\n\s*#\[schema\(example = ")[^"]+("\)\])',
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

# Both lock files record local package versions. Refresh them before the
# publish workflow checks metadata with --locked.
cargo metadata --no-deps --format-version 1 >/dev/null
cargo metadata \
    --manifest-path crates/core-timer-sensor/Cargo.toml \
    --no-deps \
    --format-version 1 >/dev/null

cargo metadata --locked --no-deps --format-version 1 >/dev/null
cargo metadata \
    --manifest-path crates/core-timer-sensor/Cargo.toml \
    --locked \
    --no-deps \
    --format-version 1 >/dev/null

echo "Updated release version to ${VERSION}."
