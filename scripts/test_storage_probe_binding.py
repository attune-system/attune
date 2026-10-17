"""Offline E/F sender-contract tests. No F script, inspector or service is run."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("storage_binding_driver", ROOT / "scripts/run-bootstrap-lifecycle-acceptance.py")
driver = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(driver)


class BindingTests(unittest.TestCase):
    def test_example_both_profiles_invoke_concrete_f_source_with_private_generated_binding(self):
        manifest = json.loads((ROOT / "scripts/acceptance/bootstrap-lifecycle.example.json").read_text())
        for profile in manifest["profiles"].values():
            self.assertEqual(profile["storage_probe_command"], ["{python}", "{source_root}/scripts/acceptance/storage_probe.py", "--binding", "{storage_binding}", "--execute"])
            self.assertFalse(profile["storage_probe"]["allow_native_faults"])
            self.assertIn(profile["action_host"], profile["storage_probe"]["writer_services"])
        args = driver.cli(["--run-id", "test"])
        self.assertEqual(driver.fixture_command(args, ["{storage_binding}"], "/private/run/probe.json"), ["/private/run/probe.json"])

    def test_sender_uses_owned_topology_distinct_credentials_and_actual_provider_values(self):
        for backend in driver.BACKENDS:
            with self.subTest(backend=backend), tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
                root = Path(directory)
                settings = {"allow_native_faults": True, "inspection_service": "worker-python", "writer_services": ["worker-python"], "poll_seconds": 180, "hold_seconds": 1800, "repository_inspector": {"binary": "/unit-test-not-executed/inspector", "sha256": "unit-test-hash"}, "aws": "/unit-test-not-executed/aws"}
                stack = SimpleNamespace(root=root, args=SimpleNamespace(run_id="test"), profile={"storage_probe": settings, "provider_service": "rustfs", "provider_port": 9000}, backend=backend, project="eaccept-test-" + backend.lower(), bucket="eaccept-test-bucket", password="unit-db-password", storage_access="unit-provider-access", storage_secret="unit-provider-secret", endpoint=lambda name, port: "127.0.0.1:" + ("12345" if name == "postgres" else "12346"))
                legacy = {"version_id": 42, "artifact_ref": "core.legacy_test", "owner_ref": "core.run_agent_command"}
                observer_file = root / "observer-input.private.json"
                driver.private_json(observer_file, {"user": "fixture_observer", "password": "unit-observer-password", "database": "attune", "schema": "attune"})
                settings["observer_credential_file"] = str(observer_file)
                with mock.patch.object(driver, "validate_storage_probe", return_value=legacy), mock.patch.object(driver.subprocess, "Popen") as process:
                    path, consent = driver.bind_storage_probe(stack, root / "maintenance-token.private", 17)
                process.assert_not_called()
                binding = json.loads(path.read_text())
                self.assertEqual((binding["project"], binding["backend"], binding["legacy_version_id"]), (stack.project, backend, 42))
                self.assertEqual(binding["maintenance_credential_file"], str(root / "maintenance-token.private"))
                self.assertEqual(binding["inspector_binary"], settings["repository_inspector"]["binary"])
                database = json.loads(Path(binding["inspector_context_file"]).read_text())
                self.assertIn("127.0.0.1:12345", database["database_url"])
                self.assertIn("unit-observer-password", database["database_url"])
                self.assertEqual(database["access"]["invoker_identity"], 17)
                self.assertEqual(database["access"]["legacy_version_ids"], [42])
                self.assertGreaterEqual(len(database["access"]["identity_key"]), 32)
                self.assertNotIn("unit-db-password", path.read_text())
                self.assertTrue(consent["allow_shell_invocation"])
                if backend == "ObjectSegments":
                    provider = json.loads(Path(binding["provider_credentials_file"]).read_text())
                    self.assertEqual(provider["bucket"], stack.bucket)
                    self.assertEqual(provider["endpoint"], "http://127.0.0.1:12346")
                    self.assertEqual(provider["secret_access_key"], stack.storage_secret)
                    self.assertEqual(binding["object_prefix"], stack.project + "/")
                    self.assertNotIn("unit-provider-secret", path.read_text())
                else:
                    self.assertNotIn("provider_credentials_file", binding)
                self.assertTrue(all(file.stat().st_mode & 0o077 == 0 for file in root.iterdir()))

    def source_fixture(self, root, source):
        scripts = root / "scripts/acceptance"
        scripts.mkdir(parents=True)
        script = scripts / "storage_probe.py"
        script.write_text(source)
        producer = scripts / "storage_probe_producer.py"
        producer.write_text("# Static source-check input, never executed\n")
        inspector = root / "inspector-unit-artifact"
        inspector.write_bytes(b"Offline hash-validation bytes, never executed")
        inspector.chmod(0o700)
        legacy = root / "legacy.private.json"
        driver.private_json(legacy, {"run_id": "test", "project": "eaccept-test-sharedfile", "backend": "SharedFile", "version_id": 42, "artifact_ref": "core.legacy_test", "owner_ref": "core.run_agent_command"})
        observer = root / "observer.private.json"
        driver.private_json(observer, {"run_id": "test", "project": "eaccept-test-sharedfile", "user": "fixture_observer", "password": "unit-observer-password", "database": "attune", "schema": "attune"})
        args = SimpleNamespace(source_root=root, run_id="test")
        profile = {"writers": ["worker-python"], "action_host": "worker-python", "storage_probe_command": ["{python}", "{source_root}/scripts/acceptance/storage_probe.py", "--binding", "{storage_binding}", "--execute"], "storage_probe": {"source_sha256": driver.digest(script), "producer_sha256": driver.digest(producer), "repository_inspector": {"binary": str(inspector), "sha256": driver.digest(inspector)}, "allow_native_faults": True, "allow_shell_invocation": True, "allow_maintenance_management": True, "inspection_service": "worker-python", "writer_services": ["worker-python"], "poll_seconds": 180, "hold_seconds": 1800, "legacy_fixture_file": str(legacy)}}
        profile["storage_probe"]["observer_credential_file"] = str(observer)
        return args, profile

    def test_old_python_sql_and_wrong_auth_paths_cannot_be_enabled_by_consent(self):
        for source, message in (("def sql(select):\n    return select\n", "Python SQL"), ('PATH="/api/v1/auth/token-login"\n', "unregistered")):
            with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
                args, profile = self.source_fixture(Path(directory), source)
                with self.assertRaisesRegex(driver.AcceptanceError, message):
                    driver.validate_storage_probe(args, "SharedFile", profile)

    def test_unconfirmed_inspector_protocol_and_foreign_legacy_evidence_fail_closed(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            args, profile = self.source_fixture(Path(directory), "# No agreed inspector binding yet\n")
            with self.assertRaisesRegex(driver.AcceptanceError, "typed inspector binding"):
                driver.validate_storage_probe(args, "SharedFile", profile)
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            args, profile = self.source_fixture(Path(directory), 'FIELDS=("inspector_binary","inspector_sha256","inspector_context_file","kind","password","/auth/login")\n')
            self.assertEqual(driver.validate_storage_probe(args, "SharedFile", profile)["version_id"], 42)
            legacy = Path(profile["storage_probe"]["legacy_fixture_file"])
            value = json.loads(legacy.read_text())
            value["project"] = "foreign-project"
            legacy.write_text(json.dumps(value))
            with self.assertRaisesRegex(driver.AcceptanceError, "different run"):
                driver.validate_storage_probe(args, "SharedFile", profile)

    def test_consent_and_artifact_hashes_are_not_success_flags(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            args, profile = self.source_fixture(Path(directory), 'FIELDS=("inspector_binary","inspector_sha256","inspector_context_file","kind","password","/auth/login")\n')
            profile["storage_probe"]["allow_shell_invocation"] = False
            with self.assertRaisesRegex(driver.AcceptanceError, "consent"):
                driver.validate_storage_probe(args, "SharedFile", profile)
            profile["storage_probe"]["allow_shell_invocation"] = True
            profile["storage_probe"]["repository_inspector"]["sha256"] = "wrong-unit-test-hash"
            with self.assertRaisesRegex(driver.AcceptanceError, "precompiled"):
                driver.validate_storage_probe(args, "SharedFile", profile)

    def test_old_token_only_f_adapter_is_rejected_before_any_process(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            args, profile = self.source_fixture(Path(directory), 'FIELDS=("inspector_binary","inspector_sha256","inspector_context_file")\n')
            with mock.patch.object(driver.subprocess, "Popen") as process:
                with self.assertRaisesRegex(driver.AcceptanceError, "maintenance password JSON"):
                    driver.validate_storage_probe(args, "SharedFile", profile)
            process.assert_not_called()


if __name__ == "__main__":
    unittest.main()
