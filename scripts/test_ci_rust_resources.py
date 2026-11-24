"""Check CI test disk policy; --dry-run reports the plan without invoking Cargo."""

import json
import subprocess
import tempfile
import sys
import tomllib
import unittest
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parent.parent
PROFILE_ENV = {
    "CARGO_PROFILE_DEV_DEBUG": "0",
    "CARGO_PROFILE_TEST_DEBUG": "0",
    "CARGO_PROFILE_TEST_LTO": "off",
    "CARGO_PROFILE_DEV_STRIP": "debuginfo",
    "CARGO_PROFILE_TEST_STRIP": "debuginfo",
    "CARGO_BUILD_JOBS": "2",
}
TEST_COMMANDS = {
    "Tests": 'scripts/run-ci-rust-tests.sh "${{ matrix.shard }}"',
    "Executor broker integration test": (
        '"${ATTUNE_BROKER_TEST_BIN:?Missing precompiled executor tests}" '
        "service::tests::test_service_creation --ignored --exact --nocapture --test-threads=4"
    ),
    "S3 storage correctness tests": (
        ': "${ATTUNE_S3_COMMON_TEST_BIN:?Missing precompiled common tests}"\n'
        ': "${ATTUNE_S3_REPLICA_TEST_BIN:?Missing precompiled replica tests}"\n'
        "make test-runtime-log-correctness\n"
    ),
}
NATIVE_TEST_COMMANDS = {
    "Standalone core action tests": "cargo test --manifest-path crates/core-actions/Cargo.toml --locked -- --test-threads=4",
    "Native HTTP action loopback HTTP and TLS tests": "python3 scripts/test_native_http_action.py",
    "Static native HTTP action loopback HTTP and TLS tests": "python3 scripts/test_native_http_action.py",
    "Native core archive smoke test": "cargo run -p attune-worker --example core_native_archive",
    "Standalone timer sensor tests": "cargo test --manifest-path crates/core-timer-sensor/Cargo.toml --locked -- --test-threads=4",
}


def inputs():
    workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
    return workflow, manifest


def resource_plan(workflow, manifest):
    job = workflow["jobs"]["rust-test"]
    build = workflow["jobs"]["rust-test-build"]
    environment = dict(workflow.get("env", {}), **build["env"])
    profiles = {}
    for profile in ("dev", "test"):
        defaults = manifest["profile"][profile]
        profiles[profile] = {
            "opt_level_unchanged": defaults["opt-level"],
            "manifest_debug": defaults["debug"],
            "ci_debug": environment.get(f"CARGO_PROFILE_{profile.upper()}_DEBUG", defaults["debug"]),
            "ci_strip": environment.get(f"CARGO_PROFILE_{profile.upper()}_STRIP", "none"),
            "ci_lto": environment.get(f"CARGO_PROFILE_{profile.upper()}_LTO", defaults.get("lto", False)),
        }
    caches = [step["with"] for step in build["steps"] + job["steps"] if step.get("uses", "").startswith("actions/cache@")]
    commands = {step["name"]: step["run"] for step in job["steps"] if step.get("name") in TEST_COMMANDS}
    return {
        "mode": "DRY_RUN_NO_CARGO_NO_INFRASTRUCTURE",
        "profiles": profiles,
        "incremental": str(environment["CARGO_INCREMENTAL"]),
        "build_jobs": str(environment.get("CARGO_BUILD_JOBS", "cargo-default")),
        "libtest_threads": str(job["env"]["ATTUNE_RUST_TEST_THREADS"]),
        "cache_paths": [entry["path"].splitlines() for entry in caches],
        "cache_restore_prefixes": [entry.get("restore-keys", "").splitlines() for entry in caches],
        "test_commands": commands,
        "workspace_runner": (ROOT / "scripts/run-ci-rust-tests.sh").read_text().splitlines()[-1],
        "release_profile_unchanged": manifest["profile"]["release"],
        "disk_capacity_verdict": "UNMEASURED: require the full remote suite and before/after disk telemetry",
    }


def validate_policy(workflow, manifest):
    plan = resource_plan(workflow, manifest)
    environment = workflow["jobs"]["rust-test-build"]["env"]
    for name, expected in PROFILE_ENV.items():
        if str(environment.get(name)) != expected:
            raise AssertionError(f"Tests job must set {name}={expected}")
        if name in workflow.get("env", {}):
            raise AssertionError("Test disk settings must not change publication/global profiles")
    if plan["incremental"] != "0":
        raise AssertionError("CI incremental compilation must remain disabled")
    allowed_paths = {"~/.cargo/registry/index", "~/.cargo/registry/cache", "~/.cargo/git/db"}
    if any(path not in allowed_paths for paths in plan["cache_paths"] for path in paths):
        raise AssertionError("Tests must not restore stale target/test/Clippy build outputs")
    if plan["workspace_runner"] != 'cargo test --workspace --all-features -- --test-threads="$threads"':
        raise AssertionError("Preserve complete all-features workspace test coverage")
    if int(plan["libtest_threads"]) < 4:
        raise AssertionError("Database tests require at least four threads")
    if plan["test_commands"] != TEST_COMMANDS:
        raise AssertionError("Keep all workspace, broker and S3 test commands unchanged")
    if any(key.startswith("CARGO_PROFILE_RELEASE_") or key.endswith("_OPT_LEVEL") for key in environment):
        raise AssertionError("Disk policy must not change release or optimization behavior")
    return plan


class CiRustResourceTests(unittest.TestCase):
    def test_ci_test_job_uses_lean_scoped_profiles_and_no_stale_build_cache(self):
        validate_policy(*inputs())

    def test_native_core_coverage_runs_in_a_parallel_job(self):
        workflow, _manifest = inputs()
        native_job = workflow["jobs"]["native-core-test"]
        self.assertEqual(native_job["needs"], "changes")
        self.assertNotIn("services", native_job)
        commands = {step["name"]: step["run"] for step in native_job["steps"]
                    if step.get("name") in NATIVE_TEST_COMMANDS}
        self.assertEqual(commands, NATIVE_TEST_COMMANDS)
        builds = next(step["run"] for step in native_job["steps"]
                      if step.get("name") == "Build static core action fixtures")
        for arch in ("x86_64", "aarch64"):
            self.assertIn(f"--target {arch}-unknown-linux-musl --bins", builds)

    def test_shards_require_the_build_and_keep_special_coverage(self):
        workflow, _manifest = inputs()
        build = workflow["jobs"]["rust-test-build"]
        shard = workflow["jobs"]["rust-test"]
        self.assertIn("rust-test-build", shard["needs"])
        self.assertEqual(shard["strategy"]["matrix"]["shard"], [1, 2])
        steps = {step["name"]: step for step in build["steps"] + shard["steps"]}
        self.assertIn("cargo test --workspace --all-features --no-run --timings",
                      steps["Compile workspace test executables"]["run"])
        self.assertEqual(steps["Workspace doctests"]["run"],
                         "cargo test --workspace --all-features --doc -- --test-threads=4")
        self.assertIn("target/debug/attune-api --config config.test.yaml --migrate",
                      steps["Initialize test database"]["run"])
        for name in ("S3 storage correctness tests", "Executor broker integration test"):
            self.assertEqual(steps[name]["if"], "matrix.shard == 1")

    def test_artifact_manifest_runs_each_test_executable_once(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "target/debug/deps").mkdir(parents=True)
            targets = (
                ("attune_common", "lib", "common"),
                ("runtime_log_replica_tests", "test", "api"),
                ("attune_executor", "lib", "executor"),
                ("test_database_lifecycle_helper", "test", "common"),
                ("attune-api", "bin", "api"),
                ("attune", "bin", "cli"),
                ("attune-mcp", "bin", "cli"),
                ("credential_storage_tests", "test", "cli"),
            )
            messages = []
            for name, kind, package in targets:
                is_test = kind != "bin"
                binary = root / ("target/debug/deps" if is_test else "target/debug") / name
                fixture = (
                    '#!/usr/bin/env python3\n'
                    'import os, sys\n'
                    'assert os.getcwd() == os.environ["CARGO_MANIFEST_DIR"]\n'
                    'assert sys.argv[1:] == ["--test-threads=4"]\n'
                )
                if name == "credential_storage_tests":
                    fixture += (
                        'assert os.access(os.environ["CARGO_BIN_EXE_attune"], os.X_OK)\n'
                        'assert os.access(os.environ["CARGO_BIN_EXE_attune-mcp"], os.X_OK)\n'
                    )
                binary.write_text(fixture + f'print("ran {name}")\n')
                binary.chmod(0o755)
                crate = root / "crates" / package
                crate.mkdir(parents=True, exist_ok=True)
                messages.append(json.dumps({
                    "reason": "compiler-artifact",
                    "profile": {"test": is_test and name != "test_database_lifecycle_helper"},
                    "target": {"name": name, "kind": [kind]},
                    "manifest_path": str(crate / "Cargo.toml"),
                    "executable": str(binary),
                }))
            helper = [sys.executable, str(ROOT / "scripts/ci-rust-test-artifacts.py")]
            paths = subprocess.run([*helper, "pack"], cwd=root, text=True,
                                   capture_output=True, check=True, input="\n".join(messages))
            self.assertIn("target/debug/attune-api", paths.stdout.splitlines())
            executions = []
            for shard in (1, 2):
                output = subprocess.check_output([*helper, "run", str(shard), "4"],
                                                 cwd=root, text=True)
                executions.extend(line for line in output.splitlines() if line.startswith("ran "))
            self.assertCountEqual(executions, [f"ran {name}" for name, kind, _ in targets if kind != "bin"])
            exported = subprocess.check_output([*helper, "env"], cwd=root, text=True)
            assignments = dict(line.split("=", 1) for line in exported.splitlines())
            self.assertEqual(set(assignments),
                             {"ATTUNE_S3_COMMON_TEST_BIN", "ATTUNE_S3_REPLICA_TEST_BIN", "ATTUNE_BROKER_TEST_BIN"})
            command = subprocess.check_output(
                ["make", "-n", "test-runtime-log-correctness",
                 *(f"{name}={value}" for name, value in assignments.items())],
                cwd=ROOT, text=True,
            )
            self.assertNotIn("cargo test", command)
            self.assertIn("--ignored --exact --test-threads=4", command)
            self.assertIn("--skip bounded_runtime_log_load_report --skip volume_transport_child_holds_lock", command)

    def test_missing_debug_override_is_rejected(self):
        workflow, manifest = inputs()
        for name in PROFILE_ENV:
            with self.subTest(variable=name):
                original = workflow["jobs"]["rust-test-build"]["env"].pop(name, None)
                with self.assertRaises(AssertionError):
                    validate_policy(workflow, manifest)
                if original is not None:
                    workflow["jobs"]["rust-test-build"]["env"][name] = original

    def test_restoring_target_or_clippy_cache_is_rejected(self):
        workflow, manifest = inputs()
        workflow["jobs"]["rust-test"]["steps"].append({
            "uses": "actions/cache@fixture", "with": {"path": "target", "restore-keys": "cargo-clippy-"},
        })
        with self.assertRaises(AssertionError):
            validate_policy(workflow, manifest)

    def test_release_checks_run_this_regression(self):
        workflow, _manifest = inputs()
        step = next(step for step in workflow["jobs"]["release-checks"]["steps"] if step.get("name") == "Validate release scripts")
        self.assertIn("python3 scripts/test_ci_rust_resources.py", step["run"])

    def test_before_and_after_disk_telemetry_is_not_cleanup_or_a_passing_capacity_claim(self):
        workflow, manifest = inputs()
        steps = {step.get("name"): step for job in ("rust-test-build", "rust-test")
                 for step in workflow["jobs"][job]["steps"]}
        for name in ("Record disk usage before Rust compilation", "Record disk usage after Rust tests"):
            command = steps[name]["run"]
            self.assertIn("df -h .", command)
            self.assertIn('du -sh "$directory"', command)
            self.assertNotIn("prune", command)
            self.assertNotIn("rm ", command)
        self.assertEqual(steps["Record disk usage after Rust tests"]["if"], "always()")
        self.assertIn("UNMEASURED", resource_plan(workflow, manifest)["disk_capacity_verdict"])


if __name__ == "__main__":
    if sys.argv[1:] == ["--dry-run"]:
        print(json.dumps(resource_plan(*inputs()), indent=2))
    else:
        unittest.main()
