#!/usr/bin/env python3
"""Extract only Cargo test executables that contain ignored integration tests."""

import json
import shutil
import subprocess
import sys
from pathlib import Path

metadata_path, messages_path, output_path = map(Path, sys.argv[1:4])
metadata = json.loads(metadata_path.read_text())
package_names = {package["id"]: package["name"] for package in metadata["packages"]}
output_path.mkdir(parents=True, exist_ok=True)
manifest = []
inventory = []
seen = set()

for line in messages_path.read_text().splitlines():
    try:
        message = json.loads(line)
    except json.JSONDecodeError:
        continue
    if message.get("reason") != "compiler-artifact" or not message.get("profile", {}).get("test"):
        continue
    executable = message.get("executable")
    package = package_names.get(message.get("package_id"))
    if not executable or not package or (package, executable) in seen:
        continue
    seen.add((package, executable))
    source = Path(executable)
    if message.get("target", {}).get("name") == "test_database_lifecycle_helper":
        destination = output_path / "test_database_lifecycle"
        shutil.copy2(source, destination)
        subprocess.run(["strip", "--strip-unneeded", destination], check=True)
        continue
    listing = subprocess.run(
        [source, "--list", "--ignored"],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout.splitlines()
    ignored_tests = [
        entry.rsplit(": ", 1)[0]
        for entry in listing
        if entry.endswith(": test") or entry.endswith(": benchmark")
    ]
    if not ignored_tests:
        continue

    package_dir = output_path / package
    package_dir.mkdir(exist_ok=True)
    destination = package_dir / source.name
    shutil.copy2(source, destination)
    subprocess.run(["strip", "--strip-unneeded", destination], check=True)
    manifest.append((package, str(destination)))
    inventory.extend((package, source.name, test) for test in ignored_tests)

if not manifest:
    raise SystemExit("cargo produced no executables containing ignored tests")

with (output_path / "manifest.tsv").open("w") as handle:
    for package, executable in sorted(manifest):
        handle.write(f"{package}\t{executable}\n")

with (output_path / "inventory.tsv").open("w") as handle:
    for package, executable, test in sorted(inventory):
        handle.write(f"{package}\t{executable}\t{test}\n")
