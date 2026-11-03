"""Check CI test disk policy; --dry-run reports the plan without invoking Cargo."""

import json
import sys
import tomllib
import unittest
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parent.parent
PROFILE_ENV = {
    "CARGO_PROFILE_DEV_DEBUG": "0",
    "CARGO_PROFILE_TEST_DEBUG": "0",
    "CARGO_PROFILE_DEV_STRIP": "debuginfo",
    "CARGO_PROFILE_TEST_STRIP": "debuginfo",
    "CARGO_BUILD_JOBS": "2",
}
TEST_COMMANDS = {
    "Tests": "scripts/run-ci-rust-tests.sh",
    "Standalone timer sensor tests": "cargo test --manifest-path crates/core-timer-sensor/Cargo.toml --locked -- --test-threads=4",
    "Executor broker integration test": "cargo test -p attune-executor --lib service::tests::test_service_creation -- --ignored --exact --nocapture --test-threads=4",
    "S3 storage correctness tests": "make test-runtime-log-correctness",
}


def inputs():
    workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
    return workflow, manifest


def resource_plan(workflow, manifest):
    job = workflow["jobs"]["rust-test"]
    environment = dict(workflow.get("env", {}), **job["env"])
    profiles = {}
    for profile in ("dev", "test"):
        defaults = manifest["profile"][profile]
        profiles[profile] = {
            "opt_level_unchanged": defaults["opt-level"],
            "manifest_debug": defaults["debug"],
            "ci_debug": environment.get(f"CARGO_PROFILE_{profile.upper()}_DEBUG", defaults["debug"]),
            "ci_strip": environment.get(f"CARGO_PROFILE_{profile.upper()}_STRIP", "none"),
        }
    caches = [step["with"] for step in job["steps"] if step.get("uses", "").startswith("actions/cache@")]
    commands = {step["name"]: step["run"] for step in job["steps"] if step.get("name") in (
        "Tests", "Standalone timer sensor tests", "Executor broker integration test", "S3 storage correctness tests")}
    return {
        "mode": "DRY_RUN_NO_CARGO_NO_INFRASTRUCTURE",
        "profiles": profiles,
        "incremental": str(environment["CARGO_INCREMENTAL"]),
        "build_jobs": str(environment.get("CARGO_BUILD_JOBS", "cargo-default")),
        "libtest_threads": str(environment["ATTUNE_RUST_TEST_THREADS"]),
        "cache_paths": [entry["path"].splitlines() for entry in caches],
        "cache_restore_prefixes": [entry.get("restore-keys", "").splitlines() for entry in caches],
        "test_commands": commands,
        "workspace_runner": (ROOT / "scripts/run-ci-rust-tests.sh").read_text().splitlines()[-1],
        "release_profile_unchanged": manifest["profile"]["release"],
        "disk_capacity_verdict": "UNMEASURED: require the full remote suite and before/after disk telemetry",
    }


def validate_policy(workflow, manifest):
    plan = resource_plan(workflow, manifest)
    environment = workflow["jobs"]["rust-test"]["env"]
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
        raise AssertionError("Keep all workspace, standalone, broker and S3 test commands unchanged")
    if any(key.startswith("CARGO_PROFILE_RELEASE_") or key.endswith("_OPT_LEVEL") for key in environment):
        raise AssertionError("Disk policy must not change release or optimization behavior")
    return plan


class CiRustResourceTests(unittest.TestCase):
    def test_ci_test_job_uses_lean_scoped_profiles_and_no_stale_build_cache(self):
        validate_policy(*inputs())

    def test_missing_debug_override_is_rejected(self):
        workflow, manifest = inputs()
        for name in PROFILE_ENV:
            with self.subTest(variable=name):
                original = workflow["jobs"]["rust-test"]["env"].pop(name, None)
                with self.assertRaises(AssertionError):
                    validate_policy(workflow, manifest)
                if original is not None:
                    workflow["jobs"]["rust-test"]["env"][name] = original

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
        steps = {step.get("name"): step for step in workflow["jobs"]["rust-test"]["steps"]}
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
