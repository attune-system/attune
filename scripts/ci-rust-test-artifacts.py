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
}

# Executable wall times from CI run 45508077350. Use stable Cargo target names
# because rustc changes executable hashes between builds.
TEST_DURATIONS = {
    "crates/api/artifact_read_performance_tests": 152.0,
    "crates/api/attune_api": 21.2,
    "crates/api/bootstrap_deployment_transaction_tests": 31.0,
    "crates/api/execution_log_stream_admission_api_tests": 11.7,
    "crates/api/pack_registry_tests": 27.4,
    "crates/api/recorded_execution_events_tests": 800.7,
    "crates/cli/test_auth": 17.0,
    "crates/common/attune_common": 32.8,
    "crates/common/cache_large_generation_integrity_tests": 205.5,
    "crates/common/execution_admission_tests": 50.7,
    "crates/common/file_evidence_retention_tests": 17.4,
    "crates/common/recorded_rollout_tests": 77.4,
    "crates/common/runtime_authority_repository_tests": 599.5,
    "crates/common/storage_maintenance_repository_tests": 12.2,
    "crates/executor/attune_executor": 80.4,
    "crates/notifier/attune_notifier": 14.6,
    "crates/supervisor/attune_supervisor": 32.7,
}
DEFAULT_TEST_DURATION = 2.0


def partition(entries):
    def duration(entry):
        package, name, _kind, _path = entry
        return TEST_DURATIONS.get(f"{package}/{name.replace('-', '_')}", DEFAULT_TEST_DURATION)

    shards = [[], []]
    totals = [0.0, 0.0]
    for entry in sorted(entries, key=lambda entry: (-duration(entry), entry)):
        index = min(range(len(shards)), key=lambda index: totals[index])
        shards[index].append(entry)
        totals[index] += duration(entry)
    return [sorted(shard) for shard in shards]


def pack(messages):
    root = Path.cwd()
    files = {MANIFEST}
    tests = {}
    binaries = {}
    for message in map(json.loads, messages):
        if message["reason"] != "compiler-artifact" or not message["executable"]:
            continue
        path = Path(message["executable"]).relative_to(root)
        target = message["target"]
        package = str(Path(message["manifest_path"]).parent.relative_to(root))
        if message["profile"]["test"] or "test" in target["kind"]:
            tests[path] = (package, target["name"], target["kind"][0], str(path))
        elif "bin" in target["kind"]:
            binaries.setdefault(package, {})[target["name"]] = str(path)
        else:
            continue
        files.add(path)

    entries = sorted(tests.values())
    if len(entries) < 2 or Path("target/debug/attune-api") not in files:
        sys.exit("Workspace build did not emit the expected test and API binaries")
    special_test_paths(entries)
    MANIFEST.write_text(json.dumps({"root": str(root), "tests": entries, "binaries": binaries}))
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


def run_shard(root, entries, binaries, shard, threads):
    selected = partition(entries)[shard - 1]
    if not selected:
        sys.exit(f"No executables assigned to shard {shard}")

    for package, _name, _kind, path in selected:
        cwd = root / package
        env = dict(os.environ, CARGO_MANIFEST_DIR=str(cwd))
        env.update({f"CARGO_BIN_EXE_{name}": str(root / binary)
                    for name, binary in binaries.get(package, {}).items()})
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
        run_shard(root, manifest["tests"], manifest["binaries"], args.shard, args.threads)


if __name__ == "__main__":
    main()
