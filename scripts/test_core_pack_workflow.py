#!/usr/bin/env python3
"""Offline shipped-workflow contracts and real wrapper tests. No action/service launches."""

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import re
import unittest
from unittest import mock
import urllib.error

import yaml

ROOT = Path(__file__).resolve().parent.parent
CORE = ROOT / "packs/core"
SPEC = importlib.util.spec_from_file_location("pack_api", CORE / "actions/pack_api.py")
pack_api = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pack_api)


class WorkflowShapeTests(unittest.TestCase):
    def setUp(self):
        self.actions = {value["ref"]: value for path in (CORE / "actions").glob("*.yaml")
                        if (value := yaml.safe_load(path.read_text()))}
        self.installer = self.actions["core.install_packs"]
        self.graph = yaml.safe_load((CORE / "actions" / self.installer["workflow_file"]).read_text())
        self.tasks = {task["name"]: task for task in self.graph["tasks"]}

    def test_shipped_secret_parameters_have_no_literal_defaults_or_domains(self):
        def inspect(schema, path):
            if isinstance(schema, dict):
                if schema.get("secret") is True:
                    self.assertFalse(set(schema) & {"default", "const", "enum", "examples"}, path)
                for name, value in schema.items():
                    inspect(value, path + "/" + str(name))
            elif isinstance(schema, list):
                for index, value in enumerate(schema):
                    inspect(value, path + "/" + str(index))

        for reference, action in self.actions.items():
            inspect(action.get("parameters", {}), reference + "/parameters")

    def test_two_file_v1_graph_has_only_real_finite_action_bindings(self):
        self.assertEqual(self.graph["version"], "1.0.0")
        self.assertEqual(set(self.graph), {"version", "vars", "tasks", "output_map"})
        self.assertFalse(set(self.installer) & {"runner_type", "entry_point", "output_schema", "tasks"})
        self.assertEqual(len(self.tasks), len(self.graph["tasks"]))
        visited = set()

        def visit(name, active):
            self.assertNotIn(name, active, "installer must have finite, nonrecursive control flow")
            if name in visited:
                return
            task = self.tasks[name]
            self.assertIn(task["action"], self.actions)
            self.assertFalse(set(task) & {"on_success", "on_failure", "on_timeout", "publish", "when"})
            action = self.actions[task["action"]]
            self.assertTrue(set(task.get("input", {})).issubset(action.get("parameters", {})))
            required = {key for key, schema in action.get("parameters", {}).items() if schema.get("required")}
            self.assertTrue(required.issubset(task.get("input", {})))
            for transition in task.get("next", []):
                self.assertIsInstance(transition.get("do", []), list)
                for target in transition.get("do", []):
                    self.assertIn(target, self.tasks)
                    visit(target, active | {name})
            visited.add(name)

        visit("initialize", set())
        self.assertEqual(visited, set(self.tasks))
        self.assertNotIn("core.run_pack_tests", self.actions)
        self.assertFalse(list((CORE / "workflows").glob("*.yaml")))

    def test_public_defaults_outputs_and_typed_parameters_are_preserved(self):
        parameters = self.installer["parameters"]
        self.assertEqual({name: parameters[name]["default"] for name in
                          ("skip_dependencies", "skip_tests", "skip_env_build", "force")},
                         dict.fromkeys(("skip_dependencies", "skip_tests", "skip_env_build", "force"), False))
        self.assertTrue(parameters["packs"]["required"])
        self.assertEqual(parameters["packs"]["items"]["type"], "string")
        self.assertEqual(parameters["packs_base_dir"]["default"], "/opt/attune/packs")
        self.assertEqual(parameters["api_url"]["default"], "http://localhost:8080")
        self.assertEqual(parameters["timeout"]["default"], 1800)
        self.assertEqual(set(self.installer["output"]),
                         {"registered_packs", "failed_packs", "installed_dependencies", "summary"})
        self.assertEqual(set(self.installer["output"]), set(self.graph["output_map"]))
        for task in self.graph["tasks"]:
            for value in task.get("input", {}).values():
                if isinstance(value, str) and "{{ parameters." in value:
                    self.assertTrue(value.startswith("{{") and value.endswith("}}"))
        text = yaml.safe_dump(self.graph)
        for legacy in (" | ", "task.download_packs.result", "uuid()", "now()", "core.run_pack_tests"):
            self.assertNotIn(legacy, text)
        # Check function names against the actual evaluator source, not a shadow
        # workflow interpreter that could accept expressions Rust rejects.
        evaluator = (ROOT / "crates/common/src/workflow/expression/evaluator.rs").read_text()
        for function in set(re.findall(r"\b([a-z_]+)\(", text)) - {"succeeded", "failed", "timed_out", "result"}:
            self.assertIn('"' + function + '" =>', evaluator)

    def test_skip_flags_testing_failure_and_secret_contracts(self):
        registration = self.tasks["register_packs"]
        self.assertEqual(registration["input"]["skip_tests"], "{{ parameters.skip_tests }}")
        self.assertEqual(registration["input"]["force"], "{{ parameters.force }}")
        self.assertIs(registration["input"]["skip_validation"], False)
        self.assertEqual(registration["next"][1]["when"], "{{ succeeded() and result().summary.success_count == 0 }}")
        self.assertEqual(registration["next"][1]["do"], ["fail_installation"])
        self.assertEqual(self.tasks["fail_installation"]["action"], "core.noop")
        self.assertEqual(self.tasks["fail_installation"]["input"]["exit_code"], 1)
        self.assertNotIn("next", self.tasks["fail_installation"])
        for name, flag, taken, skipped in (
            ("check_dependencies", "skip_dependencies", "get_dependencies", "check_environment"),
            ("check_environment", "skip_env_build", "build_environments", "register_packs"),
        ):
            branches = self.tasks[name]["next"]
            self.assertEqual(branches[0]["when"], "{{ succeeded() and not parameters." + flag + " and not ('api_token' in parameters) }}")
            self.assertEqual(branches[0]["do"], [taken])
            self.assertEqual(branches[1]["when"], "{{ succeeded() and not parameters." + flag + " and 'api_token' in parameters }}")
            self.assertEqual(branches[1]["do"], [taken + "_with_token"])
            self.assertEqual(branches[2]["do"], [skipped])
            if name == "check_environment":
                self.assertEqual(branches[3]["do"], [skipped + "_with_token"])
        self.assertTrue(self.installer["parameters"]["api_token"]["secret"])
        for name in pack_api.OPERATIONS:
            action = self.actions["core." + name]
            self.assertEqual(action["parameter_format"], "json")
            self.assertEqual(action["parameter_delivery"], "stdin")
            self.assertEqual(action["required_worker_runtimes"], {"python": ">=3.11,<4.0"})
            self.assertTrue(action["parameters"]["api_token"]["secret"])
            self.assertNotIn("default", action["parameters"]["api_url"])
            task_name = {"get_pack_dependencies": "get_dependencies", "build_pack_envs": "build_environments"}.get(name, name)
            omitted = self.tasks[task_name]
            provided = self.tasks[task_name + "_with_token"]
            self.assertNotIn("api_token", omitted["input"])
            self.assertNotIn("api_url", omitted["input"])
            self.assertEqual(provided["input"]["api_token"], "{{ parameters.api_token }}")
            self.assertEqual(provided["input"]["api_url"], "{{ parameters.api_url }}")
            self.assertEqual({key: value for key, value in provided["input"].items() if key not in ("api_token", "api_url")}, omitted["input"])
            self.assertEqual(provided["next"], omitted["next"])
            self.assertNotIn("default_execution_permission_set_refs", action)
            self.assertNotIn("output_schema", action)
        # Secrets are inputs only. No output or publish expression may include them.
        self.assertNotIn("api_token", yaml.safe_dump(self.graph["output_map"]))
        for task in self.graph["tasks"]:
            for transition in task.get("next", []):
                self.assertNotIn("api_token", yaml.safe_dump(transition.get("publish", [])))

    def test_canonical_output_schemas_accept_actual_nullable_api_fields(self):
        downloaded = self.actions["core.download_packs"]["output"]["downloaded_packs"]["items"]["properties"]
        for name in ("git_commit", "checksum"):
            self.assertEqual(downloaded[name]["type"], ["string", "null"])
        registered = self.actions["core.register_packs"]["output"]["registered_packs"]["items"]["properties"]
        self.assertEqual(registered["test_result"]["type"], ["object", "null"])
        runtime = self.actions["core.get_pack_dependencies"]["output"]["runtime_requirements"]["additionalProperties"]["properties"]
        for name in ("python", "nodejs"):
            self.assertEqual(runtime[name]["type"], ["object", "null"])
        environments = self.actions["core.build_pack_envs"]["output"]["built_environments"]["items"]["properties"]["environments"]["properties"]
        for name in ("python", "nodejs"):
            self.assertEqual(environments[name]["type"], ["object", "null"])


class PackApiTests(unittest.TestCase):
    def invoke(self, operation, parameters, result):
        opener = mock.MagicMock()
        opener.open.return_value.__enter__.return_value.read.return_value = json.dumps({"data": result}).encode()
        with mock.patch.object(pack_api.urllib.request, "build_opener", return_value=opener) as build:
            output = pack_api.call(operation, parameters)
        proxy, redirect = build.call_args.args
        self.assertEqual(proxy.proxies, {})
        self.assertIsInstance(redirect, pack_api.NoRedirect)
        self.assertIsNone(redirect.redirect_request(None, None, 302, "redirect", {}, "https://other.invalid"))
        return output, opener.open.call_args.args[0]

    def test_download_real_json_arrays_quoting_and_result_paths(self):
        sources = ['https://example.invalid/a?x="quoted"', "https://example.invalid/β.tar.gz"]
        output, request = self.invoke("download_packs", {
            "packs": sources, "destination_dir": "/tmp/owned\nquoted'path", "ref_spec": "",
            "api_url": "https://api.invalid", "api_token": "owned-secret", "verify_ssl": True,
        }, {"downloaded_packs": [{"pack_path": "/tmp/returned/path"}], "failed_packs": [], "success_count": 1})
        body = json.loads(request.data)
        self.assertEqual(body["packs"], sources)
        self.assertEqual(body["destination_dir"], "/tmp/owned\nquoted'path")
        self.assertNotIn("ref_spec", body)
        self.assertNotIn("api_token", body)
        self.assertEqual(request.get_header("Authorization"), "Bearer owned-secret")
        self.assertEqual(output["pack_paths"], ["/tmp/returned/path"])
        self.assertIsInstance(output["duration_seconds"], int)

    def test_each_real_endpoint_preserves_boolean_and_array_body_types(self):
        for name, (endpoint, _) in pack_api.OPERATIONS.items():
            if name == "download_packs":
                continue
            result = {"summary": {"success_count": 1, "failure_count": 0}, "registered_packs": [],
                      "failed_packs": [], "failed_environments": [], "detection_only": True,
                      "missing_dependencies": [], "errors": []}
            body = {"pack_paths": ["/tmp/a", "/tmp/b"], "api_token": "owned-secret"}
            if name == "register_packs":
                body.update(force=True, skip_tests=False)
            output, request = self.invoke(name, body, result)
            self.assertTrue(request.full_url.endswith("/api/v1/packs/" + endpoint))
            self.assertEqual(json.loads(request.data)["pack_paths"], body["pack_paths"])
            if name == "register_packs":
                self.assertIs(json.loads(request.data)["skip_tests"], False)
            self.assertEqual(output["summary"], result["summary"])

    def test_invalid_inputs_fail_before_any_request(self):
        for values in (["not-object"], {"pack_paths": "not-array"}, {"pack_paths": []},
                       {"pack_paths": ["x"], "force": "false"}, {"pack_paths": ["x"], "unknown": 1},
                       {"pack_paths": ["x"], "api_url": "https://name:secret@api.invalid"}):
            if isinstance(values, dict):
                values = dict(values, api_token="owned-secret")
            with self.subTest(values=values), mock.patch.object(pack_api.urllib.request, "build_opener") as build:
                with self.assertRaises(ValueError):
                    pack_api.call("register_packs", values)
                build.assert_not_called()

    def test_unauthenticated_call_denies_without_manufacturing_authority(self):
        with mock.patch.dict(pack_api.os.environ, {}, clear=True), mock.patch.object(pack_api.urllib.request, "build_opener") as build:
            with self.assertRaisesRegex(ValueError, "existing bearer"):
                pack_api.call("register_packs", {"pack_paths": ["/tmp/a"]})
            build.assert_not_called()

    def test_recorded_context_token_is_used_only_when_explicit_bearer_absent(self):
        with mock.patch.dict(pack_api.os.environ, {"ATTUNE_API_TOKEN": "recorded-token", "ATTUNE_API_URL": "https://api.invalid"}, clear=True):
            _, request = self.invoke("get_pack_dependencies", {"pack_paths": ["/tmp/a"]}, {"missing_dependencies": [], "errors": []})
        self.assertEqual(request.get_header("Authorization"), "Bearer recorded-token")
        self.assertTrue(request.full_url.startswith("https://api.invalid/"))

    def test_explicit_bearer_endpoint_does_not_disclose_the_context_credential(self):
        with mock.patch.dict(pack_api.os.environ, {"ATTUNE_API_TOKEN": "recorded-token", "ATTUNE_API_URL": "https://context.invalid"}, clear=True):
            _, request = self.invoke("get_pack_dependencies", {
                "pack_paths": ["/tmp/a"], "api_url": "https://explicit.invalid", "api_token": "provided-token",
            }, {"missing_dependencies": [], "errors": []})
        self.assertEqual(request.get_header("Authorization"), "Bearer provided-token")
        self.assertTrue(request.full_url.startswith("https://explicit.invalid/"))
        self.assertNotIn(b"provided-token", request.data)
        self.assertNotIn(b"recorded-token", request.data)

    def test_context_token_cannot_be_redirected_by_an_api_url_parameter(self):
        with mock.patch.dict(pack_api.os.environ, {"ATTUNE_API_TOKEN": "recorded-token", "ATTUNE_API_URL": "https://api.invalid"}, clear=True):
            with mock.patch.object(pack_api.urllib.request, "build_opener") as build:
                with self.assertRaisesRegex(ValueError, "context API URL"):
                    pack_api.call("get_pack_dependencies", {"pack_paths": ["/tmp/a"], "api_url": "https://other.invalid"})
                build.assert_not_called()

    def test_http_failure_never_logs_body_or_credential_and_never_retries(self):
        error = urllib.error.HTTPError("https://api.invalid", 403, "owned-secret", {}, io.BytesIO(b"owned-secret"))
        stderr, stdout = io.StringIO(), io.StringIO()
        with mock.patch.object(pack_api, "call", side_effect=error) as call, mock.patch.object(pack_api.sys, "stdin", io.StringIO('{"api_token":"owned-secret"}')):
            with contextlib.redirect_stderr(stderr), contextlib.redirect_stdout(stdout):
                self.assertEqual(pack_api.main("register_packs"), 1)
        self.assertTrue(error.fp.closed)
        call.assert_called_once()
        self.assertIn("HTTP 403", stderr.getvalue())
        self.assertNotIn("owned-secret", stderr.getvalue() + stdout.getvalue())

    def test_malformed_or_oversized_responses_are_not_reported_as_success(self):
        for result in ({}, {"summary": {"success_count": True, "failure_count": 0}, "registered_packs": [], "failed_packs": []}):
            with self.assertRaises(ValueError):
                self.invoke("register_packs", {"pack_paths": ["/tmp/a"], "api_token": "owned-secret"}, result)
        with mock.patch.object(pack_api, "MAX_RESPONSE_BYTES", 2):
            with self.assertRaisesRegex(ValueError, "size limit"):
                self.invoke("register_packs", {"pack_paths": ["/tmp/a"], "api_token": "owned-secret"}, {})


if __name__ == "__main__":
    unittest.main()
