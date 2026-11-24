"""Package Cargo test executables and run a disjoint shard on a hosted runner."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time


MANIFEST = Path("target/ci-test-executables.json")
SPECIAL_TESTS = {
    ("attune_common", "lib"): "ATTUNE_S3_COMMON_TEST_BIN",
    ("runtime_log_replica_tests", "test"): "ATTUNE_S3_REPLICA_TEST_BIN",
    ("attune_executor", "lib"): "ATTUNE_BROKER_TEST_BIN",
}


def pack(messages):
    root = Path.cwd()
    files = {MANIFEST}
    tests = {}
    for message in map(json.loads, messages):
        if message["reason"] != "compiler-artifact" or not message["executable"]:
            continue
        path = Path(message["executable"]).relative_to(root)
        target = message["target"]
        if message["profile"]["test"] or "test" in target["kind"]:
            package = Path(message["manifest_path"]).parent.relative_to(root)
            tests[path] = (str(package), target["name"], target["kind"][0], str(path))
        elif "bin" not in target["kind"]:
            continue
        files.add(path)

    entries = sorted(tests.values())
    if len(entries) < 2 or Path("target/debug/attune-api") not in files:
        sys.exit("Workspace build did not emit the expected test and API binaries")
    special_test_paths(entries)
    MANIFEST.write_text(json.dumps({"root": str(root), "tests": entries}))
    for path in sorted(files):
        if not path.is_file() or path.is_symlink():
            sys.exit(f"Expected a regular Cargo artifact at {path}")
        print(path)


def special_test_paths(entries):
    paths = {}
    for _package, name, kind, path in entries:
        variable = SPECIAL_TESTS.get((name, kind))
        if variable:
            if variable in paths:
                sys.exit(f"Multiple test executables for {variable}")
            paths[variable] = path
    if paths.keys() != set(SPECIAL_TESTS.values()):
        sys.exit(f"Missing test executables for {set(SPECIAL_TESTS.values()) - paths.keys()}")
    return paths


def run_shard(root, entries, shard, threads):
    selected = entries[shard - 1::2]
    if not selected:
        sys.exit(f"No executables assigned to shard {shard}")

    for package, _name, _kind, path in selected:
        cwd = root / package
        env = dict(os.environ, CARGO_MANIFEST_DIR=str(cwd))
        print(f"::group::Running {package}/{Path(path).name}", flush=True)
        start = time.monotonic()
        status = subprocess.run([root / path, f"--test-threads={threads}"], cwd=cwd, env=env).returncode
        print("::endgroup::", flush=True)
        print(f"{package}/{Path(path).name} took {time.monotonic() - start:.1f}s", flush=True)
        if status:
            sys.exit(status if status > 0 else 128 - status)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    subcommands.add_parser("pack")
    subcommands.add_parser("env")
    run = subcommands.add_parser("run")
    run.add_argument("shard", type=int, choices=(1, 2))
    run.add_argument("threads", type=int)
    args = parser.parse_args()

    if args.command == "pack":
        pack(sys.stdin)
        return
    root = Path.cwd()
    manifest = json.loads(MANIFEST.read_text())
    if root != Path(manifest["root"]):
        sys.exit("Test artifact checkout path differs from the build checkout path")
    if args.command == "env":
        for variable, path in special_test_paths(manifest["tests"]).items():
            print(f"{variable}={root / path}")
    else:
        run_shard(root, manifest["tests"], args.shard, args.threads)


if __name__ == "__main__":
    main()
