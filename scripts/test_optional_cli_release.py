"""Offline regression checks for optional CLI release failure isolation."""

import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

import yaml


ROOT = Path(__file__).resolve().parent.parent
JOBS = yaml.safe_load((ROOT / ".github/workflows/publish.yml").read_text())["jobs"]
OPTIONAL = {
    "submit-macos-cli-notarization",
    "finalize-macos-cli-notarization",
    "build-windows-cli",
    "build-linux-packages",
    "sign-arch-packages",
    "publish-homebrew-cask",
    "publish-arch-package-repository",
    "publish-linux-packages-nexus",
}
REQUIRED = {
    "metadata", "release-gate", "build-rust-bundles", "build-docker-dist",
}


def condition_matches(condition, results, *, cancelled=False, tag=True, outputs=None, stable=True):
    expression = condition.strip().removeprefix("${{").removesuffix("}}").strip()
    expression = expression.replace("!cancelled()", str(not cancelled))
    expression = expression.replace("github.ref_type", repr("tag" if tag else "branch"))
    expression = expression.replace("!contains(github.ref_name, '-')", str(stable))
    expression = re.sub(
        r"needs\.([\w-]+)\.result",
        lambda match: repr(results.get(match[1], "success")), expression,
    )
    expression = re.sub(
        r"needs\.publish-github-release\.outputs\.(\w+)",
        lambda match: repr("true" if (outputs or {}).get(match[1], False) else "false"), expression,
    )
    expression = expression.replace("&&", " and ").replace("||", " or ")
    # Only evaluate the small boolean expression left after resolving known inputs.
    if not re.fullmatch(r"[\s\w'()=!-]+", expression):
        raise AssertionError(f"Unrecognized release condition: {expression}")
    return eval(expression, {"__builtins__": {}}, {})


class OptionalCliReleaseTests(unittest.TestCase):
    def test_only_cli_distribution_jobs_allow_failure(self):
        self.assertEqual(
            {name for name, job in JOBS.items() if job.get("continue-on-error") is True},
            OPTIONAL,
        )

    def test_release_runs_after_optional_failure_but_never_after_required_failure(self):
        condition = JOBS["publish-github-release"]["if"]
        self.assertIn("!cancelled()", condition)
        for result in ("failure", "skipped", "success"):
            with self.subTest(optional_result=result):
                self.assertTrue(condition_matches(condition, dict.fromkeys(OPTIONAL, result)))
        for job in REQUIRED:
            for result in ("failure", "cancelled", "skipped"):
                with self.subTest(required_job=job, result=result):
                    self.assertFalse(condition_matches(condition, {job: result}))
        self.assertFalse(condition_matches(condition, {}, cancelled=True))
        self.assertFalse(condition_matches(condition, {}, tag=False))

    def test_server_publishers_do_not_depend_on_optional_jobs(self):
        def ancestors(name):
            dependencies = JOBS[name].get("needs", [])
            if isinstance(dependencies, str):
                dependencies = [dependencies]
            return set(dependencies).union(*(ancestors(item) for item in dependencies))

        for job in ("publish-rust-images", "publish-rust-manifests", "publish-web-images", "publish-web-manifest"):
            with self.subTest(job=job):
                self.assertTrue(OPTIONAL.isdisjoint(ancestors(job)))
                self.assertIn("release-gate", ancestors(job))

    def test_artifact_inventory_handles_missing_and_partial_platforms(self):
        step = next(step for step in JOBS["publish-github-release"]["steps"] if step.get("id") == "cli")
        cases = (
            ([], {}),
            (["attune-windows-cli-amd64"], {"windows": "true"}),
            (["attune-macos-cli-arm64"], {"macos": "true"}),
            (["attune-macos-cli-amd64", "attune-macos-cli-arm64"], {"macos": "true", "macos_complete": "true"}),
            (["attune-macos-notarization-amd64", "attune-macos-notarization-arm64"], {}),
            (["attune-linux-cli-amd64"], {"linux_cli": "true"}),
            (["attune-linux-cli-amd64", "attune-linux-cli-arm64"], {"linux_cli": "true", "linux_cli_complete": "true"}),
            (["attune-linux-packages-amd64", "attune-linux-packages-arm64"], {}),
            (["attune-linux-packages-amd64", "attune-linux-packages-arm64", "attune-linux-package-signatures-amd64"], {}),
            (["attune-linux-packages-amd64", "attune-linux-packages-arm64", "attune-linux-package-signatures-amd64",
              "attune-linux-package-signatures-arm64"], {"linux_packages": "true"}),
        )
        for names, expected in cases:
            with self.subTest(artifacts=names), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                # This stub only supplies artifact names to the real workflow shell.
                gh = root / "gh"
                gh.write_text('#!/bin/sh\nprintf "%s\\n" "$TEST_ARTIFACT_NAMES"\n')
                gh.chmod(0o700)
                output = root / "output"
                summary = root / "summary"
                environment = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                                   TEST_ARTIFACT_NAMES="\n".join(names), GITHUB_OUTPUT=str(output),
                                   GITHUB_STEP_SUMMARY=str(summary), REPOSITORY="attune-system/attune", RUN_ID="1")
                subprocess.run(["bash", "-euo", "pipefail", "-c", step["run"]], env=environment, check=True)
                values = dict(line.split("=", 1) for line in output.read_text().splitlines())
                defaults = dict.fromkeys(("windows", "macos", "macos_complete", "linux_cli", "linux_cli_complete", "linux_packages"), "false")
                self.assertEqual(values, {**defaults, **expected})
                self.assertIn("optional", summary.read_text().lower())

    def test_downloads_are_guarded_and_release_verification_explicitly_opts_in(self):
        steps = {step.get("name"): step for step in JOBS["publish-github-release"]["steps"]}
        self.assertEqual(steps["Download Windows CLI archive"]["if"], "steps.cli.outputs.windows == 'true'")
        self.assertEqual(steps["Download macOS CLI archives"]["if"], "steps.cli.outputs.macos == 'true'")
        self.assertEqual(steps["Download Linux CLI archives"]["if"], "steps.cli.outputs.linux_cli == 'true'")
        for name in ("Download Linux packages", "Download Arch package signatures"):
            self.assertEqual(steps[name]["if"], "steps.cli.outputs.linux_packages == 'true'")
        self.assertNotIn("if", steps["Download binary bundles"])
        self.assertIn("--allow-missing-cli", steps["Verify release asset families"]["run"])
        self.assertIn("--allow-missing-linux-packages", steps["Verify release asset families"]["run"])

    def test_linux_archive_packaging_does_not_fail_the_required_runtime_bundle(self):
        steps = {step.get("name"): step for step in JOBS["build-rust-bundles"]["steps"]}
        self.assertTrue(steps["Package Linux CLI archive"]["continue-on-error"])
        self.assertTrue(steps["Upload Linux CLI artifact"]["continue-on-error"])
        self.assertEqual(steps["Upload Linux CLI artifact"]["if"], "steps.package-cli.outcome == 'success'")
        self.assertNotIn("continue-on-error", steps["Build static agent binaries"])
        self.assertIn("--bin attune-mcp", steps["Build static agent binaries"]["run"])

    def test_homebrew_requires_both_finalized_macos_archives_and_a_public_release(self):
        condition = JOBS["publish-homebrew-cask"]["if"]
        outputs = {"macos_complete": True}
        self.assertTrue(condition_matches(condition, {}, outputs=outputs))
        self.assertFalse(condition_matches(condition, {}))
        self.assertFalse(condition_matches(condition, {"publish-github-release": "failure"}, outputs=outputs))
        self.assertFalse(condition_matches(condition, {"release-gate": "failure"}, outputs=outputs))

    def test_linux_repositories_skip_missing_archives_packages_or_signatures(self):
        for job, output in (("publish-arch-package-repository", "linux_cli_complete"),
                            ("publish-linux-packages-nexus", "linux_packages")):
            with self.subTest(job=job):
                condition = JOBS[job]["if"]
                self.assertTrue(condition_matches(condition, {}, outputs={output: True}))
                self.assertFalse(condition_matches(condition, {}))
                self.assertFalse(condition_matches(condition, {"publish-github-release": "failure"}, outputs={output: True}))

    def test_release_checks_include_this_suite(self):
        ci = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
        commands = "\n".join(step.get("run", "") for step in ci["jobs"]["release-checks"]["steps"])
        self.assertIn("python3 scripts/test_optional_cli_release.py", commands)


if __name__ == "__main__":
    unittest.main()
