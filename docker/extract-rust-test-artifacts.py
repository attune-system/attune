#!/usr/bin/env python3
"""Extract Cargo test executables that contain normally runnable tests."""

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
binary_manifest = []
inventory = []
seen = set()
seen_binaries = set()

for line in messages_path.read_text().splitlines():
    try:
        message = json.loads(line)
    except json.JSONDecodeError:
        continue
    if message.get("reason") != "compiler-artifact":
        continue
    executable = message.get("executable")
    package = package_names.get(message.get("package_id"))
    if not executable or not package:
        continue
    target = message.get("target", {})
    if "bin" in target.get("kind", []) and not message.get("profile", {}).get("test"):
        binary_name = target.get("name")
        if binary_name and binary_name not in seen_binaries:
            seen_binaries.add(binary_name)
            binary_dir = output_path / "bin"
            binary_dir.mkdir(exist_ok=True)
            destination = binary_dir / binary_name
            shutil.copy2(Path(executable), destination)
            subprocess.run(["strip", "--strip-unneeded", destination], check=True)
            binary_manifest.append((binary_name, str(destination)))
        continue
    if not message.get("profile", {}).get("test") or (package, executable) in seen:
        continue
    seen.add((package, executable))
    source = Path(executable)
    if target.get("name") == "test_database_lifecycle_helper":
        destination = output_path / "test_database_lifecycle"
        shutil.copy2(source, destination)
        subprocess.run(["strip", "--strip-unneeded", destination], check=True)
        continue
    listing = subprocess.run(
        [source, "--list"],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout.splitlines()
    ignored_listing = subprocess.run(
        [source, "--list", "--ignored"],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout.splitlines()
    all_tests = [
        entry.rsplit(": ", 1)[0]
        for entry in listing
        if entry.endswith(": test") or entry.endswith(": benchmark")
    ]
    if not all_tests:
        continue
    ignored_tests = {
        entry.rsplit(": ", 1)[0]
        for entry in ignored_listing
        if entry.endswith(": test") or entry.endswith(": benchmark")
    }
    runnable_tests = [
        test for test in all_tests if test not in ignored_tests
    ]

    package_dir = output_path / package
    package_dir.mkdir(exist_ok=True)
    destination = package_dir / source.name
    shutil.copy2(source, destination)
    subprocess.run(["strip", "--strip-unneeded", destination], check=True)
    manifest.append((package, str(destination)))
    inventory.extend((package, source.name, test) for test in runnable_tests)

if not manifest:
    raise SystemExit("cargo produced no test executables")

with (output_path / "manifest.tsv").open("w") as handle:
    for package, executable in sorted(manifest):
        handle.write(f"{package}\t{executable}\n")

with (output_path / "binaries.tsv").open("w") as handle:
    for name, executable in sorted(binary_manifest):
        handle.write(f"{name}\t{executable}\n")

with (output_path / "inventory.tsv").open("w") as handle:
    for package, executable, test in sorted(inventory):
        handle.write(f"{package}\t{executable}\t{test}\n")
