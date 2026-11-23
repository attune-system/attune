"""Offline coverage of real workflow conditions and final CI gate rejection paths."""

import ast
import copy
import itertools
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import unittest

import yaml

from check_ci_gate import ADVISORY, JOBS, PATH_FLAGS, evaluate


ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())


def expression_value(expression, event, flags, inputs, context=None):
    """Interpret the workflow's boolean/string expressions without executing code."""
    expression = expression.strip().removeprefix("${{").removesuffix("}}").strip()
    values = {"github.event_name": event}
    values.update({f"needs.changes.outputs.{key}": value for key, value in flags.items()})
    values.update(context or {})
    for _, key in JOBS.values():
        if key:
            values[f"inputs.{key}"] = inputs.get(key, False)
    expression = re.sub(
        r"(?:github\.[\w.]+|env\.\w+|needs\.changes\.outputs\.\w+|inputs\.\w+)",
        lambda match: repr(values[match[0]]), expression,
    ).replace("&&", " and ").replace("||", " or ")

    def resolve(node):
        if isinstance(node, ast.Constant) and type(node.value) in (str, bool):
            return node.value
        if isinstance(node, ast.BoolOp):
            values = [resolve(value) for value in node.values]
            result = values[0]
            for value in values[1:]:
                if isinstance(node.op, ast.And):
                    result = result and value
                else:
                    result = result or value
            return result
        if isinstance(node, ast.Compare) and len(node.ops) == 1:
            left, right = resolve(node.left), resolve(node.comparators[0])
            if isinstance(node.ops[0], ast.Eq):
                return left == right
            if isinstance(node.ops[0], ast.NotEq):
                return left != right
        raise AssertionError(f"Unsupported workflow expression: {expression}")

    return resolve(ast.parse(" ".join(expression.split()), mode="eval").body)


def changed_path_flags(*paths):
    completed = subprocess.run(
        ["bash", str(ROOT / "scripts/ci-changed-paths.sh")],
        input="\n".join(paths) + "\n", capture_output=True, text=True,
        check=True, timeout=10,
    )
    return dict(line.split("=", 1) for line in completed.stdout.splitlines())


def results(event="pull_request", flags=None, inputs=None):
    flags = flags if flags is not None else dict.fromkeys(PATH_FLAGS, "true")
    inputs = inputs if inputs is not None else {}
    needs = {"changes": {"result": "success", "outputs": flags}}
    for name in JOBS:
        condition = WORKFLOW["jobs"][name]["if"]
        expected = expression_value(condition, event, flags, inputs)
        needs[name] = {"result": "success" if expected else "skipped"}
    return needs


class CiGateTests(unittest.TestCase):
    def test_web_runtime_paths_select_the_image_smoke_job(self):
        web_runtime = {"web_image"}
        rust_infrastructure = {"rust", "release", "smoke"}
        for paths, enabled in (
            (("docker/README.md",), set()),
            (("docker/Dockerfile.web", "docker/README.md"), web_runtime),
            (("docker/nginx.conf",), web_runtime),
            (("docker/inject-env.sh",), web_runtime),
            ((".dockerignore",), web_runtime | {"release", "smoke"}),
            (("docker/Dockerfile.rust-tests",), rust_infrastructure),
            (("docker/Dockerfile.rust-tests", "docker/Dockerfile.web"),
             web_runtime | rust_infrastructure),
        ):
            with self.subTest(paths=paths):
                flags = changed_path_flags(*paths)
                self.assertEqual(flags, {key: str(key in enabled).lower() for key in PATH_FLAGS})
                needs = results(flags=flags)
                self.assertEqual(evaluate(needs, "pull_request", {}), ([], []))
                self.assertEqual(needs["web-image"]["result"],
                                 "success" if "web_image" in enabled else "skipped")
                self.assertEqual(needs["rust-test"]["result"],
                                 "success" if "rust" in enabled else "skipped")

    def test_cargo_deny_input_paths(self):
        for path in ("Cargo.toml", "Cargo.lock", "crates/api/Cargo.toml",
                     "crates/core-actions/Cargo.lock", "tests/sqlx-session/Cargo.toml",
                     "vendor/sqlx-core-0.8.6/Cargo.toml", "deny.toml", ".cargo/config",
                     ".cargo/config.toml"):
            with self.subTest(path=path):
                flags = changed_path_flags(path)
                self.assertEqual(flags["cargo_deny_inputs"], "true")
                self.assertEqual(results(flags=flags)["rust-audit"]["result"], "success")
        for path in ("crates/api/src/lib.rs", "docs/Cargo.toml.md", "web/package-lock.json"):
            with self.subTest(path=path):
                self.assertEqual(changed_path_flags(path)["cargo_deny_inputs"], "false")

    def test_cargo_deny_failures_are_advisory_for_prs_without_input_changes(self):
        job = WORKFLOW["jobs"]["rust-audit"]
        policy = job["env"]["CARGO_DENY_ADVISORY"]
        steps = {step["id"]: step for step in job["steps"] if "id" in step}
        for event, ref, head, merge_base, changed, advisory in (
            ("pull_request", "refs/pull/1/merge", "feature", "", "false", True),
            ("pull_request", "refs/pull/1/merge", "feature", "", "true", False),
            ("pull_request", "refs/pull/1/merge", "feature", "", "", False),
            ("pull_request", "refs/pull/1/merge", "main", "", "false", False),
            ("push", "refs/heads/main", "", "", "false", False),
            ("merge_group", "refs/heads/gh-readonly-queue/main/pr-1", "",
             "refs/heads/main", "false", False),
        ):
            with self.subTest(event=event, ref=ref, head=head, changed=changed):
                flags = {"cargo_deny_inputs": changed}
                context = {"github.ref": ref, "github.head_ref": head,
                           "github.event.merge_group.base_ref": merge_base}
                self.assertEqual(expression_value(policy, event, flags, {}, context), advisory)
                context["env.CARGO_DENY_ADVISORY"] = str(advisory).lower()
                for step in ("deny-workspace", "deny-core-actions", "deny-timer"):
                    self.assertEqual(expression_value(steps[step]["continue-on-error"],
                                                      event, flags, {}, context), advisory)

    def test_every_path_combination_matches_the_real_push_and_pr_conditions(self):
        for event in ("pull_request", "push"):
            for values in itertools.product(("true", "false"), repeat=len(PATH_FLAGS)):
                flags = dict(zip(sorted(PATH_FLAGS), values))
                needs = results(event, flags)
                with self.subTest(event=event, flags=flags):
                    self.assertEqual(evaluate(needs, event, {}), ([], []))
                    for name in JOBS:
                        if needs[name]["result"] == "success":
                            broken = copy.deepcopy(needs)
                            broken[name]["result"] = "skipped"
                            self.assertTrue(evaluate(broken, event, {})[0], name)

    def test_every_manual_selection_matches_real_workflow_conditions(self):
        trigger = WORKFLOW.get("on", WORKFLOW.get(True))
        declarations = trigger["workflow_dispatch"]["inputs"]
        keys = sorted(declarations)
        self.assertEqual(set(keys), {key for _, key in JOBS.values() if key})
        for declaration in declarations.values():
            self.assertEqual(declaration["type"], "boolean")
        for values in itertools.product((True, False), repeat=len(keys)):
            inputs = dict(zip(keys, values))
            needs = results("workflow_dispatch", dict.fromkeys(PATH_FLAGS, "false"), inputs)
            with self.subTest(inputs=inputs):
                self.assertEqual(evaluate(needs, "workflow_dispatch", inputs), ([], []))
        # Release configuration remains mandatory even with all input flags off.
        needs["release-checks"]["result"] = "skipped"
        self.assertTrue(evaluate(needs, "workflow_dispatch", inputs)[0])

    def test_failures_cancellations_and_missing_results_cannot_pass_silently(self):
        for name in JOBS:
            for result in ("failure", "cancelled", "skipped", "unknown", None):
                needs = results()
                needs[name]["result"] = result
                with self.subTest(job=name, result=result):
                    errors, warnings = evaluate(needs, "pull_request", {})
                    advisory_failure = name in ADVISORY and result == "failure"
                    self.assertEqual(bool(errors), not advisory_failure)
                    self.assertEqual(bool(warnings), advisory_failure)

    def test_failed_or_skipped_path_detection_rejects_the_run(self):
        for result in ("failure", "cancelled", "skipped", None):
            needs = results()
            needs["changes"]["result"] = result
            self.assertTrue(evaluate(needs, "pull_request", {})[0])

    def test_malformed_or_incomplete_context_fails_closed(self):
        for mutation in (
            lambda needs: needs.pop("rust-test"),
            lambda needs: needs.update({"new-unaccounted-job": {"result": "success"}}),
            lambda needs: needs["changes"]["outputs"].pop("rust"),
            lambda needs: needs["changes"]["outputs"].update({"rust": "maybe"}),
            lambda needs: needs.update({"rust-test": None}),
        ):
            needs = results()
            mutation(needs)
            with self.assertRaises(ValueError):
                evaluate(needs, "pull_request", {})
        with self.assertRaises(ValueError):
            evaluate(results(), "unsupported-event", {})
        with self.assertRaises(ValueError):
            evaluate(results(), "workflow_dispatch", {})
        inputs = {key: True for _, key in JOBS.values() if key}
        inputs["run_rust_test"] = "false"
        with self.assertRaises(ValueError):
            evaluate(results(), "workflow_dispatch", inputs)

    def test_gate_waits_for_all_jobs_and_manual_runs_use_a_distinct_check(self):
        self.assertEqual(set(WORKFLOW["jobs"]["changes"]["outputs"]), PATH_FLAGS)
        gate = WORKFLOW["jobs"]["ci-gate"]
        self.assertEqual(set(gate["needs"]), set(WORKFLOW["jobs"]) - {"ci-gate"})
        self.assertEqual(set(gate["needs"]), set(JOBS) | {"changes"})
        self.assertEqual(gate["if"].strip(), "always()")
        self.assertFalse(gate.get("continue-on-error", False))
        for event in ("pull_request", "push", "workflow_dispatch"):
            name = expression_value(gate["name"], event, {}, {})
            self.assertEqual(name, "CI Gate (manual)" if event == "workflow_dispatch" else "CI Gate")
        advisory = {name for name, job in WORKFLOW["jobs"].items() if job.get("continue-on-error") is True}
        self.assertEqual(advisory, ADVISORY)
        step = next(step for step in gate["steps"] if step.get("name") == "Evaluate CI results")
        self.assertEqual(step["env"], {
            "CI_NEEDS": "${{ toJSON(needs) }}",
            "CI_EVENT": "${{ github.event_name }}",
            "CI_INPUTS": "${{ toJSON(inputs) }}",
        })
        self.assertEqual(step["run"], "python3 scripts/check_ci_gate.py")
        validation = next(step for step in WORKFLOW["jobs"]["release-checks"]["steps"]
                          if step.get("name") == "Validate release scripts")
        self.assertIn("python3 scripts/test_ci_gate.py", validation["run"])

    def test_cli_reports_real_exit_status_for_pass_fail_and_invalid_json(self):
        failed = results()
        failed["rust-test"]["result"] = "failure"
        advisory = results()
        advisory["security-advisory"]["result"] = "failure"
        for needs, expected in ((results(), 0), (failed, 1), (advisory, 0), ({}, 1), ("invalid json", 1)):
            completed = subprocess.run(
                [sys.executable, str(ROOT / "scripts/check_ci_gate.py")],
                env={**os.environ, "CI_EVENT": "pull_request", "CI_INPUTS": "{}",
                     "CI_NEEDS": needs if isinstance(needs, str) else json.dumps(needs)},
                capture_output=True, text=True, timeout=10,
            )
            self.assertEqual(completed.returncode, expected, completed.stdout + completed.stderr)


if __name__ == "__main__":
    unittest.main()
