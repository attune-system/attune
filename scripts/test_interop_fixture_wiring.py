"""E/G source integration tests only; no SDK, archive helper or browser is run."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
import yaml

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("interop_wiring_driver", ROOT / "scripts/run-bootstrap-lifecycle-acceptance.py")
driver = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(driver)
interop = driver.interoperability_module()


class Wiring(unittest.TestCase):
    def test_fixture_contains_exact_worker_and_one_retained_source_not_plaintext(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            source = root / "source/scripts/acceptance"
            source.mkdir(parents=True)
            worker = b"# offline byte-copy boundary, never executed\n"
            for name in interop.WORKERS:
                (source / name).write_bytes(worker + name.encode())
            principals = {"pack_ref": "ginterop_test", "source_key_ref": "identity:test.sponsor:key"}
            pack = interop.compose(root / "source", root / "fixture", principals, driver)
            for name in interop.WORKERS:
                self.assertEqual((pack / "actions" / name).read_bytes(), worker + name.encode())
            actions = [yaml.safe_load(path.read_text()) for path in (pack / "actions").glob("*.yaml")]
            self.assertEqual({a["ref"] for a in actions}, {"ginterop_test." + name for name in ("probe", "input_probe", "input_workflow", "emit", "batch_probe", "external_probe")})
            defaults = [field["default"] for action in actions for field in action["parameters"].values() if "default" in field]
            self.assertEqual(defaults, ["{{ keystore['identity:test.sponsor:key'] }}"])
            for action in actions:
                if "workflow_file" in action:
                    self.assertNotIn("runner_type", action)
                    continue
                self.assertEqual((action["parameter_delivery"], action["parameter_format"], action["output_format"]), ("stdin", "json", "json"))
                expected = ["ginterop_test.runtime"] if action["ref"].split(".")[-1] in ("probe", "input_probe") else ["ginterop_test.emitter"] if action["ref"].endswith(".emit") else []
                self.assertEqual(action["default_execution_permission_set_refs"], expected)
            config = yaml.safe_load((pack / "pack.yaml").read_text())["config"]
            self.assertIs(type(config["interop_counter"]), int)
            queue = yaml.safe_load((pack / "queues/inbox.yaml").read_text())
            self.assertTrue(queue["enabled"])
            self.assertTrue(queue["accepting_new_items"])
            self.assertEqual(queue["batch_mode"], "batch")
            self.assertEqual(queue["config"]["dispatch"]["batch_size"], {"source": "literal", "value": 2})
            self.assertFalse(queue["config"]["dispatch"]["coalescing"]["enabled"])
            self.assertEqual(queue["action_params"], {"queue_items": "{{ queue_items }}"})
            self.assertEqual(queue["permission_set_refs"], [])
            self.assertTrue(queue["item_schema"]["secret"]["secret"])
            self.assertTrue((pack / "types/probe_receipt.yaml").exists())
            self.assertEqual(yaml.safe_load((pack / "permission_sets/runtime.yaml").read_text())["scope"], "pack")
            trigger = yaml.safe_load((pack / "triggers/input_tick.yaml").read_text())
            self.assertEqual(set(trigger["output"]), {"nonce", "runtime_action_ref"})
            for path, key, prefix in (("queues/input_inbox.yaml", "action_params", "item"), ("rules/input_rule.yaml", "action_params", "event.payload")):
                component = yaml.safe_load((pack / path).read_text())
                self.assertEqual(component["permission_set_refs"], [])
                self.assertEqual(component[key], {field: "{{ " + prefix + "." + field + " }}" for field in ("nonce", "runtime_action_ref")})
                self.assertNotIn("input", component[key])
            graph = yaml.safe_load((pack / "actions/workflows/input_workflow.workflow.yaml").read_text())
            self.assertEqual(len(graph["tasks"]), 1)
            self.assertEqual(graph["tasks"][0]["permission_set_refs"], [])
            self.assertNotIn("input", graph["tasks"][0]["input"])
            sets = {p.stem: yaml.safe_load(p.read_text()) for p in (pack / "permission_sets").glob("*.yaml")}
            self.assertEqual(set(sets), {"runtime", "runtime_alt", "external_runtime", "emitter"})
            self.assertEqual(sets["external_runtime"]["grants"], sets["runtime"]["grants"])
            self.assertEqual(sets["emitter"]["grants"], [{"resource": "events", "actions": ["create"], "constraints": {"refs": ["ginterop_test.input_tick"]}}])
            for path in (pack / "types").glob("*.yaml"):
                schema = yaml.safe_load(path.read_text())["schema"]
                self.assertIsInstance(schema["required"], list)
                self.assertTrue(all("required" not in field for field in schema["properties"].values()))
            probe = next(a for a in actions if a["ref"].endswith(".probe"))
            self.assertIn("runtime_value_sha256", probe["output"])

    def test_invalid_source_ref_or_existing_pack_cannot_be_overwritten(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            with self.assertRaises(driver.AcceptanceError):
                interop.compose(ROOT, Path(directory) / "new", {"pack_ref": "ginterop_test", "source_key_ref": "x'] }}"}, driver)
            with self.assertRaises(driver.AcceptanceError):
                interop.compose(ROOT, Path(directory), {"pack_ref": "ginterop_test", "source_key_ref": "identity:test:key"}, driver)

    def test_stage_binds_actual_loopback_observer_and_measured_release(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            source = root / "source/scripts/acceptance"
            source.mkdir(parents=True)
            for name in interop.WORKERS:
                (source / name).write_text("# byte fixture, never executed\n")
            principals_file = root / "principals.private.json"
            principals = {"run_id": "test", "project": "eaccept-test-sharedfile", "api_url": "http://127.0.0.1:1234", "pack_ref": "ginterop_test", "source_key_ref": "identity:test:key"}
            driver.private_json(principals_file, principals)
            credential = root / "observer.private.json"
            driver.private_json(credential, {"user": "g_reader", "password": "fixture:@/?", "schema": "attune"})
            archive = root / "canonical.tar.gz"
            archive.write_bytes(b"offline command transport fixture, not an acceptance archive")
            settings = {"observer_credential_file": str(credential), "archive_tool": {"binary": "/owned/tool"}, "repository_inspector": {"binary": "/owned/interoperability_inspector"}, "transports": {}, "browser": {}, "artifacts": {}, "timeout_seconds": 7200}
            context = {key: principals[key] for key in ("run_id", "project", "api_url")}
            outcome = {"real_server_matrix_completed": True, "interoperability_criteria_passed": True, "lanes": ["api", "cli", "mcp", "python", "javascript", "java"], "browser": True, "full_sow_acceptance": False}
            fixture = mock.Mock(side_effect=[{"principals_binding_file": str(principals_file)}, outcome])
            command = mock.Mock(return_value=json.dumps({"archive_path": str(archive), "digest": driver.digest(archive)}))
            tools = SimpleNamespace(**vars(driver.fixture_tools()))
            tools.fixture, tools.command = fixture, command
            stack = SimpleNamespace(root=root, args=SimpleNamespace(source_root=root / "source"), profile={"operator_credential_file": "/owned/operator"}, backend="SharedFile", endpoint=lambda service, port: "127.0.0.1:2345" if service == "postgres" else "127.0.0.1:3456")
            with mock.patch.object(interop, "validate", return_value=settings):
                self.assertEqual(interop.stage(stack, context, tools), outcome)
            binding = driver.private_input(root / "g-interoperability-binding.private.json")
            template_path = Path(binding["inspector_context_template_file"])
            template = driver.private_input(template_path)
            self.assertEqual((template["run_id"], template["project"]), (context["run_id"], context["project"]))
            self.assertEqual(template["database_url"], "postgresql://g_reader:fixture%3A%40%2F%3F@127.0.0.1:2345/attune")
            self.assertEqual(set(template), {"run_id", "project", "database_url", "schema"})
            self.assertEqual(binding["artifacts"][str(template_path)], driver.digest(template_path))
            self.assertEqual(binding["inspector_release_digests"], [driver.digest(archive)])
            self.assertEqual(binding["inspector_binary"], "/owned/interoperability_inspector")
            self.assertNotIn("receipt_key", template)
            self.assertNotIn("access", template)
            self.assertEqual(fixture.call_args.kwargs["timeout"], 7200)
            self.assertEqual(binding["browser"]["web_url"], "http://127.0.0.1:3456")

    def test_current_runner_awaits_g_before_stop_and_protects_failed_host_children(self):
        source = (ROOT / "scripts/run-bootstrap-lifecycle-acceptance.py").read_text()
        self.assertLess(source.index("interop = interoperability_module().stage"), source.index("stack.stop_writers()  # F preflight"))
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            args = driver.cli(["--work-root", str(Path(directory) / "new"), "--run-id", "test"])
            stack = driver.Stack(args, {}, "SharedFile", {})
            stack.created = True
            stack.external_stage_unjoined = True
            with mock.patch.object(stack, "stop_writers"), mock.patch.object(stack, "compose") as compose:
                with self.assertRaisesRegex(driver.AcceptanceError, "joined-child/browser"):
                    stack.cleanup()
                compose.assert_not_called()

    def test_preflight_requires_exact_observer_owner_and_pinned_inspector(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            sources = root / "scripts/acceptance"
            sources.mkdir(parents=True)
            artifacts = {}
            for name in interop.G_SOURCES:
                file = sources / name
                file.write_bytes(b"offline source fixture only")
                artifacts[str(file)] = driver.digest(file)
            tool = root / "archive_helper"
            inspector = root / "interoperability_inspector"
            for file in (tool, inspector):
                file.write_bytes(b"offline artifact fixture only; never execute")
                file.chmod(0o700)
            artifacts[str(inspector)] = driver.digest(inspector)
            operator = root / "operator.private.json"
            driver.private_json(operator, {"kind": "password", "owned_project": "eaccept-test-sharedfile"})
            observer = root / "observer.private.json"
            observer_data = {"run_id": "test", "project": "eaccept-test-sharedfile", "user": "g_select_only", "password": "offline fixture", "database": "attune", "schema": "attune"}
            driver.private_json(observer, observer_data)
            settings = {"timeout_seconds": 7200, "archive_tool": {"binary": str(tool), "sha256": driver.digest(tool)}, "repository_inspector": {"binary": str(inspector), "sha256": driver.digest(inspector)}, "observer_credential_file": str(observer), "artifacts": artifacts, "transports": {name: {} for name in ("cli", "mcp", "python", "javascript", "java")}}
            inputs = root / "inputs.private.json"
            driver.private_json(inputs, settings)
            args = SimpleNamespace(source_root=root, run_id="test")
            profile = {"interoperability_inputs_file": str(inputs), "operator_credential_file": str(operator)}
            self.assertEqual(interop.validate(args, profile, driver, "SharedFile"), settings)
            for changed, message in (({"user": "postgres"}, "ordinary SELECT-only"), ({"user": "attune"}, "ordinary SELECT-only"), ({"project": "foreign"}, "different owned"), ({"schema": "foreign"}, "ordinary SELECT-only")):
                observer.write_text(json.dumps(dict(observer_data, **changed)))
                with self.assertRaisesRegex(driver.AcceptanceError, message):
                    interop.validate(args, profile, driver, "SharedFile")
            observer.write_text(json.dumps(observer_data))
            inspector.write_bytes(b"changed binary")
            with self.assertRaisesRegex(driver.AcceptanceError, "pinned centrally compiled"):
                interop.validate(args, profile, driver, "SharedFile")

    def test_full_stage_deadline_and_prerequisites_are_not_quiet_skips(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            file = Path(directory) / "inputs.private.json"
            driver.private_json(file, {"timeout_seconds": 60})
            with self.assertRaisesRegex(driver.AcceptanceError, "2..4 hour"):
                interop.validate(SimpleNamespace(), {"interoperability_inputs_file": str(file)}, driver)
        recipe = (ROOT / "docker/Dockerfile.acceptance-artifacts").read_text()
        self.assertIn("--example interoperability_acceptance_archive", recipe)
        self.assertIn("--example interoperability_inspector", recipe)
        helper = (ROOT / "crates/common/examples/interoperability_acceptance_archive.rs").read_text()
        self.assertIn("PackStorage::new(output).publish_release", helper)


if __name__ == "__main__":
    unittest.main()
