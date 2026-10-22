#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
mode="${1:---check}"

case "$mode" in
  --check|--fix) ;;
  *) echo "Usage: $0 [--check|--fix]" >&2; exit 2 ;;
esac

exec python3 - "$ROOT" "$mode" <<'PY'
from pathlib import Path
import re
import sys

root = Path(sys.argv[1])
mode = sys.argv[2]
pattern = re.compile(
    r'\s*#\[ignore = "(?:integration test .* requires database|'
    r'integration test - requires PostgreSQL|requires disposable PostgreSQL|'
    r'e2e test requires PostgreSQL)"\]\s*'
    r'|\s*#\[ignore\]\s*// Requires database\s*'
)
excluded = root / "crates/common/src/repositories/pack_test.rs"
webhook = root / "crates/api/tests/webhook_security_tests.rs"
try:
    crates = root / "crates"
    if not crates.is_dir():
        raise OSError(f"Rust source directory is missing: {crates}")
    # os.walk propagates traversal failures through onerror, unlike rglob.
    import os

    def scan_error(error):
        raise error

    failed = False
    for directory, _, names in os.walk(crates, onerror=scan_error):
        for name in sorted(names):
            path = Path(directory) / name
            if path.suffix != ".rs" or path == excluded:
                continue
            lines = path.read_text().splitlines(keepends=True)
            if mode == "--fix":
                retained = [line for line in lines if not pattern.fullmatch(line)
                            and not (path == webhook and re.fullmatch(r'\s*#\[ignore\]\s*', line))]
                if retained != lines:
                    path.write_text("".join(retained))
                lines = retained
            for number, line in enumerate(lines, 1):
                if pattern.fullmatch(line):
                    print(f"{path}:{number}:{line.rstrip()}")
                    failed = True
    if failed:
        print("ERROR: database-only #[ignore] attributes remain", file=sys.stderr)
        sys.exit(1)
except (OSError, UnicodeError) as error:
    print(f"ERROR: cannot scan database-test ignore policy: {error}", file=sys.stderr)
    sys.exit(2)
PY
