#!/usr/bin/env python3
"""Run real artifact API query-growth gates on owned stock PostgreSQL servers.

Reuses the native PostgreSQL fixture and the API's TestContext. Requires cached
postgres:16-alpine/18-alpine images and the Rust toolchain. No existing database,
fixed host port, secret parameter logging, or deployment configuration is used.

Examples:
  python3 scripts/benchmark-artifact-reads.py --dry-run --output /tmp/opencode/artifact-baseline
  python3 scripts/benchmark-artifact-reads.py --stage baseline --versions 18 --output /tmp/opencode/artifact-baseline
  python3 scripts/benchmark-artifact-reads.py --stage treatment --versions 16 18 --output /tmp/opencode/artifact-treatment

A red baseline exits nonzero and retains its evidence. Passing query-growth gates
are not latency certification or a substitute for native-install verification.
"""

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import shutil
from pathlib import Path
import subprocess
import sys
import uuid

from artifact_read_reports import compare_paired_reference, summarize_cardinality, summarize_explain, summarize_workload

ROOT = Path(__file__).resolve().parents[1]
TEST = "artifact_read_performance_tests"


def snapshot(root=None):
    root = ROOT if root is None else root
    paths = set()
    for pattern in (
        "crates/**/src/**/*.rs", "crates/**/Cargo.toml", "crates/**/build.rs",
        "crates/api/tests/**/*.rs", "crates/*/tests/fixtures/**/*",
        "migrations/*.sql", "crates/api/src/**/*.rs", "crates/common/src/**/*.rs",
        "crates/api/tests/artifact_read_performance_tests.rs", "crates/api/tests/artifact_read_performance/**/*.rs", "crates/api/tests/helpers.rs",
        "crates/api/tests/artifact_workload/**/*.rs", "crates/api/tests/runtime_log_replica_tests.rs",
        "crates/notifier/src/**/*.rs", "crates/notifier/Cargo.toml",
        "crates/api/tests/key_usage_tests.rs",
        "Cargo.toml", "Cargo.lock", "Makefile", "crates/api/Cargo.toml", "crates/common/Cargo.toml",
        "config.test.yaml", "docker-compose.yaml", ".github/workflows/ci.yml",
        ".sqlx/*.json", ".cargo/config.toml", "tests/fixtures/**/*", "packs/core/**/*",
        "scripts/benchmark-artifact-reads.py", "scripts/probe-postgresql-native-maintenance.py",
        "scripts/artifact_read_reports.py",
        "scripts/artifact_read_capacity.py",
        "scripts/runtime-log-s3.sh", "scripts/test-runtime-log-harness-safety.sh",
    ):
        paths.update(root.glob(pattern))
    return {
        str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(paths) if path.is_file()
        and not any(part in {"__pycache__", "node_modules", "target", ".venv", "venvs"}
                    for part in path.relative_to(root).parts)
    }


def load_protocol(root=None):
    root = ROOT if root is None else root
    spec = importlib.util.spec_from_file_location(
        "artifact_postgresql_fixture", root / "scripts/probe-postgresql-native-maintenance.py"
    )
    protocol = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(protocol)
    return protocol


def compile_test(test=TEST, package="attune-api", target_kind="--test", *, root=None, build_revision=None, target_dir=None):
    root = ROOT if root is None else root
    env = {**os.environ, "SQLX_OFFLINE": "true"}
    if build_revision is not None:
        env["ATTUNE_BUILD_GIT_SHA"] = build_revision
    if target_dir is not None:
        env["CARGO_TARGET_DIR"] = str(target_dir)
    result = subprocess.run(
        ["cargo", "test", "-p", package, target_kind, test, "--no-run", "--message-format=json"],
        cwd=root, env=env,
        capture_output=True, text=True, timeout=2400,
    )
    if result.returncode:
        rendered = []
        for line in result.stdout.splitlines():
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if message.get("reason") == "compiler-message":
                rendered.append(message["message"].get("rendered", message["message"]["message"]))
        raise RuntimeError(f"Cannot compile {test}:\n{result.stderr}\n{''.join(rendered)}")
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact"
                and message["target"]["name"] == test and message.get("executable")):
            return message["executable"]
    raise RuntimeError(f"Cargo did not report an executable for {test}")


def capture_sources(destination, expected):
    """Retain a run-owned source copy, not a Git worktree or a deployment."""
    if destination.resolve().is_relative_to(ROOT.resolve()):
        raise ValueError("source snapshot must be outside the checkout to avoid recursive copies")
    destination.mkdir(mode=0o700)
    ignored = shutil.ignore_patterns(".git", "target", "node_modules", "__pycache__", ".venv", "venvs",
        ".env", ".env.*", "credentials", "credentials.toml")
    for name in ("Cargo.toml", "Cargo.lock", "config.test.yaml", "docker-compose.yaml", "Makefile",
                 "crates", "migrations", "scripts", "tests/fixtures", "packs/core", ".sqlx",
                 ".github/workflows/ci.yml", ".cargo/config.toml", "docker", "packaging"):
        source = ROOT / name
        target = destination / name
        if not source.exists():
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.is_dir():
            shutil.copytree(source, target, symlinks=True, ignore=ignored)
        else:
            shutil.copy2(source, target)
    copied = snapshot(destination)
    if copied != expected:
        changed = sorted(name for name in copied.keys() | expected.keys() if copied.get(name) != expected.get(name))
        raise RuntimeError(f"source snapshot does not match capture: {', '.join(changed[:10])}; no tests started")
    return destination


def capture_executable(source, destination):
    """Keep shared-target rebuilds from replacing a run's selected executable."""
    source = Path(source)
    digest = hashlib.sha256()
    with source.open('rb') as reader, destination.open('xb') as writer:
        before = os.fstat(reader.fileno())
        while chunk := reader.read(1024 * 1024):
            writer.write(chunk)
            digest.update(chunk)
        after = source.stat()
        if (before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_ino, after.st_size, after.st_mtime_ns):
            raise RuntimeError("test executable changed during capture; no tests started")
    destination.chmod(0o500)
    return {"path": str(destination), "sha256": digest.hexdigest(), "bytes": before.st_size}


def main():
    cli = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    cli.add_argument("--output", required=True, type=Path, help="unused evidence directory; never overwritten")
    cli.add_argument("--versions", nargs="+", choices=("16", "18"), default=["16", "18"])
    cli.add_argument("--stage", choices=("baseline", "treatment"), default="baseline")
    cli.add_argument("--with-key-usage-tests", action="store_true", help="also run existing recorded-key and sensor disclosure regressions")
    cli.add_argument("--workload-profile", action="store_true", help="also run the explicit 1/7/30/90-partition, prepared-batch, and maintenance/capacity stress profile")
    cli.add_argument("--paired-reference", action="store_true", help="require identical serial-reference/batched responses, 90%% fewer queries, and 70%% lower warm p95 on the same owned fixture; not a historical-build comparison")
    cli.add_argument("--with-notifier-tests", action="store_true", help="also run notifier authorization, broadcast-loss, and real PostgreSQL listener-gap tests")
    cli.add_argument("--notifier-profile", action="store_true", help="also measure real notifier visibility SQL and per-identity fan-out for 1/16/64 identities with 1/4 tabs; includes notifier tests")
    cli.add_argument("--snapshot-source", action="store_true", help="build a hash-checked source copy under the unused output directory; retain it as evidence without copying build caches or local credentials")
    cli.add_argument("--dry-run", action="store_true", help="show source fingerprint and cases without building or starting Docker")
    args = cli.parse_args()
    if args.output.exists() or args.output.is_symlink():
        cli.error("--output already exists; choose a new run-owned directory")
    args.output = args.output.absolute()
    if not args.output.parent.is_dir():
        cli.error("--output parent must already exist")
    sources = snapshot()
    revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT,
                              capture_output=True, text=True, check=True).stdout.strip()
    evidence = {
        "stage": args.stage, "revision": revision, "source_sha256": sources,
        "versions": args.versions, "test_executable": TEST,
        "with_key_usage_tests": args.with_key_usage_tests,
        "workload_profile": args.workload_profile,
        "paired_reference": args.paired_reference,
        "with_notifier_tests": args.with_notifier_tests,
        "notifier_profile": args.notifier_profile,
        "snapshot_source": args.snapshot_source,
        "artifact_candidate_batch_size": 256, "execution_disclosure_batch_size": 64,
        "measurement_scope": "request-scoped SQLx statement events and real Axum responses",
        "source_frontier_query_gates": {"batch_size": 64, "query_budget_basis": "application statements excluding exact driver discovery and pool setup", "statements_at_64": 25,
                                        "statements_at_65": 40, "within_batch_source_reads": "constant"},
        "host": {"system": platform.system(), "machine": platform.machine(), "logical_cpus": os.cpu_count()},
        "limitations": ["no network/browser TTFB", "no fixed-source baseline p95 certification",
                         "pool acquisition includes connection creation/health checks, not only queue wait",
                         "Linux poll CPU is request/disclosure work, not a database CPU measurement"],
        "cases": [], "passed": False,
    }
    if args.dry_run:
        print(json.dumps(evidence, indent=2))
        return 0
    args.output.mkdir(mode=0o700)
    source_root = ROOT
    result = 1
    try:
        if args.snapshot_source:
            source_root = capture_sources(args.output / "source", sources)
            evidence["source_snapshot_path"] = str(source_root)
        target_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).absolute()
        if args.snapshot_source:
            # A shared target directory can return an executable overwritten by
            # another checkout's build before capture. Isolate each frozen build.
            target_dir = target_dir / "artifact-benchmarks" / args.output.name
        evidence["target_directory"] = str(target_dir)
        build_options = {"root": source_root, "build_revision": revision, "target_dir": target_dir}
        def build(test=TEST, package="attune-api", target_kind="--test"):
            executable = compile_test(test, package, target_kind, **build_options)
            if args.snapshot_source:
                binaries = args.output / "bin"
                binaries.mkdir(mode=0o700, exist_ok=True)
                captured = capture_executable(executable, binaries / test)
                evidence.setdefault("executables", {})[test] = captured
                executable = captured["path"]
            return executable

        artifact_executable = build()
        if args.workload_profile or args.paired_reference:
            listing = subprocess.run([artifact_executable, "--list"], cwd=source_root,
                env=os.environ.copy(), capture_output=True, text=True, check=True, timeout=30).stdout
            expected_tests = []
            if args.workload_profile:
                expected_tests.extend(f"source_origins::workload::{test}" for test in (
                    "artifact_partition_and_maintenance_workload_profile", "artifact_cardinality_and_history_workload_profile"))
            if args.paired_reference:
                expected_tests.append("reference::paired_serial_reference_latency_and_query_gate")
            for test in expected_tests:
                if f"{test}: test" not in listing:
                    raise RuntimeError(f"built executable does not contain required workload test {test}; no database tests started")
        executables = [("tests.log", artifact_executable, [])]
        if args.with_key_usage_tests:
            executables.append(("key-usage-tests.log", build("key_usage_tests"), []))
        if args.with_notifier_tests or args.notifier_profile:
            notifier = build("attune-notifier", "attune-notifier", "--bin")
            executables.append(("notifier-tests.log", notifier, []))
            if args.notifier_profile:
                executables.append(("notifier-profile.log", notifier, ["--ignored","--exact",
                    "websocket_server::tests::artifact_load::artifact_notification_authorization_load_profile"]))
        if args.workload_profile:
            executables.append(("workload-profile.log", artifact_executable, ["--ignored", "--exact",
                "source_origins::workload::artifact_partition_and_maintenance_workload_profile"]))
            executables.append(("cardinality-profile.log", artifact_executable, ["--ignored", "--exact",
                "source_origins::workload::artifact_cardinality_and_history_workload_profile"]))
        if args.paired_reference:
            executables.append(("paired-reference.log", artifact_executable, ["--ignored", "--exact",
                "reference::paired_serial_reference_latency_and_query_gate"]))
        protocol = load_protocol(source_root)
        for version in dict.fromkeys(args.versions):
            case = {"major": version, "passed": False}
            evidence["cases"].append(case)
            run_id = "artifact-" + uuid.uuid4().hex[:12]
            with protocol.Server(f"postgres:{version}-alpine", run_id, case) as server:
                server.sql("CREATE DATABASE attune_test;")
                port = case["port"].rsplit(":", 1)[1]
                output = args.output / f"pg{version}"
                output.mkdir()
                env = {**os.environ, "SQLX_OFFLINE": "true", "ATTUNE_TEST_RUN_ID": "a" + uuid.uuid4().hex[:18],
                       "CARGO_MANIFEST_DIR": str(source_root / "crates/api"),
                       "ATTUNE__DATABASE__URL": f"postgresql://postgres@127.0.0.1:{port}/attune_test",
                       "ATTUNE_ARTIFACT_BENCHMARK_OUTPUT": str(output)}
                case["test_exit_codes"] = []
                for filename, executable, selection in executables:
                    env["ATTUNE_TEST_RUN_ID"] = "a" + uuid.uuid4().hex[:18]
                    with (output / filename).open("x") as log:
                        tests = subprocess.run([executable, *selection, "--nocapture", "--test-threads=4"],
                                               cwd=source_root, env=env, stdout=log, stderr=subprocess.STDOUT,
                                               timeout=2400)
                    case["test_exit_codes"].append(tests.returncode)
                summaries = {}
                for path in output.glob("*-plan.json"):
                    summaries[path.name] = summarize_explain(json.loads(path.read_text()))
                profile_path = output / "artifact-workload-profile.json"
                if args.workload_profile and all(code == 0 for code in case["test_exit_codes"]) and not profile_path.exists():
                    raise RuntimeError("selected workload test did not produce its report; zero selected tests is not passing evidence")
                if profile_path.exists():
                    profile = json.loads(profile_path.read_text())
                    summaries["event_partition_profiles"] = [
                        {"daily_partitions": item["daily_partitions"],
                         **summarize_explain(item["prepared_event_plan"]["plan"])}
                        for item in profile["partition_profiles"]
                    ]
                    (output / "workload-summary.json").write_text(json.dumps(summarize_workload(profile), indent=2) + "\n")
                cardinality_path = output / "artifact-cardinality-profile.json"
                if args.workload_profile and all(code == 0 for code in case["test_exit_codes"]) and not cardinality_path.exists():
                    raise RuntimeError("selected cardinality test did not produce its report")
                if cardinality_path.exists():
                    (output / "cardinality-summary.json").write_text(json.dumps(summarize_cardinality(json.loads(cardinality_path.read_text())), indent=2) + "\n")
                (output / "plan-work-summary.json").write_text(json.dumps(summaries, indent=2) + "\n")
                case["owned_postgresql_volume_df_kib"] = protocol.command("docker", "exec", server.name,
                    "df", "-Pk", server.sql("SHOW data_directory;")).stdout.splitlines()
                remaining = server.sql("SELECT datname FROM pg_database WHERE datname LIKE 'attune_db_%' ORDER BY datname;")
                case["remaining_fixture_databases"] = remaining.splitlines() if remaining else []
                case["passed"] = all(code == 0 for code in case["test_exit_codes"]) and not remaining
                if args.notifier_profile and not (output / "notifier-authorization-profile.json").exists():
                    raise RuntimeError("notifier profile selection did not produce its report")
                if args.paired_reference:
                    paired = output / "paired-serial-reference.json"
                    if not paired.exists():
                        raise RuntimeError("paired reference selection did not produce its report")
                    acceptance = compare_paired_reference(json.loads(paired.read_text()), snapshot(source_root) == sources)
                    (output / "paired-reference-acceptance.json").write_text(json.dumps(acceptance, indent=2) + "\n")
                    case["paired_reference_acceptance"] = acceptance
                    case["passed"] = case["passed"] and acceptance['passed']
    except Exception as error:
        evidence["error"] = str(error)
    finally:
        after = snapshot(source_root)
        evidence["source_changes"] = sorted(name for name in sources.keys() | after.keys()
                                            if sources.get(name) != after.get(name))
        evidence["source_unchanged"] = not evidence["source_changes"]
        live = snapshot()
        evidence["live_source_changes_since_capture"] = sorted(name for name in sources.keys() | live.keys()
            if sources.get(name) != live.get(name))
        evidence["passed"] = (not evidence.get("error") and evidence["source_unchanged"]
            and bool(evidence["cases"]) and all(case["passed"] and not case.get("cleanup_errors") for case in evidence["cases"]))
        result = 0 if evidence["passed"] else 1
        (args.output / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps({"passed": evidence["passed"], "stage": args.stage,
                      "evidence": str(args.output / "evidence.json")}, indent=2))
    return result


if __name__ == "__main__":
    sys.exit(main())
