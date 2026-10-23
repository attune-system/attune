"""Source/offline safety checks. These do not run any acceptance infrastructure."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("lifecycle_runner", ROOT / "scripts/run-bootstrap-lifecycle-acceptance.py")
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class RunnerTests(unittest.TestCase):
    def test_live_negative_requires_the_current_approval_boundary_not_generic_forbidden(self):
        for status, body, accepted in ((403, "External runtime scope requires separate approval by an authorized owner or administrator", True), (403, "Not authorized to upload", False), (400, "External runtime scope requires separate approval", False)):
            stream = io.BytesIO(body.encode())
            error = runner.urllib.error.HTTPError("http://127.0.0.1:1234/api/v1/packs/upload", status, "unit test", {}, stream)
            if accepted:
                runner.require_external_approval_denial(error)
            else:
                with self.assertRaises(runner.AcceptanceError):
                    runner.require_external_approval_denial(error)
            self.assertTrue(stream.closed)

    def test_http_failure_after_changed_pins_is_recorded_without_retry_or_rollback(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            publisher = mock.Mock()
            publisher.upload.side_effect = OSError("unit-only simulated lost response")
            before = {"core": {"release_id": 1, "policy_revision": 2}, "config": {"max_action_timeout": 77}}
            after = {"core": {"release_id": 3, "policy_revision": 4}, "config": {"max_action_timeout": 77}}
            with mock.patch.object(runner, "snapshot", return_value=after):
                with self.assertRaises(OSError):
                    runner.upload_outcome(Path(directory), "upgrade", publisher, "http://127.0.0.1:1234", "unit-secret-not-live", ROOT / "packs/core", "unit-digest", before)
            publisher.upload.assert_called_once()
            record = (Path(directory) / "upgrade.outcome.private.json").read_text()
            outcome = json.loads(record)
            self.assertEqual(outcome["observed_state"], "changed")
            self.assertEqual(outcome["after"], after)
            self.assertNotIn("unit-secret-not-live", record)
            self.assertNotIn("unit-only simulated lost response", record)

    def test_unobservable_failed_upload_is_unknown_not_success_or_rollback(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            publisher = mock.Mock()
            publisher.upload.side_effect = OSError("unit unavailable")
            with mock.patch.object(runner, "snapshot", side_effect=OSError("unit unavailable")):
                with self.assertRaises(OSError):
                    runner.upload_outcome(Path(directory), "upgrade", publisher, "http://127.0.0.1:1234", "not-live", ROOT / "packs/core", None, {})
            outcome = json.loads((Path(directory) / "upgrade.outcome.private.json").read_text())
            self.assertEqual(outcome["observed_state"], "unavailable")
            self.assertNotIn("after", outcome)
            publisher.upload.assert_called_once()

    def args(self, root):
        return runner.cli(["--run-id", "eaoffline", "--work-root", str(root / "new-run")])

    def test_default_is_a_no_process_no_resource_plan(self):
        output = io.StringIO()
        with mock.patch.object(runner.subprocess, "Popen") as process, contextlib.redirect_stdout(output):
            self.assertEqual(runner.main(["--run-id", "eaoffline"]), 0)
        process.assert_not_called()
        plan = json.loads(output.getvalue())
        self.assertEqual(plan["mode"], "DRY_RUN")
        self.assertEqual(plan["max_build_jobs"], 2)
        self.assertEqual(plan["required_profiles"], ["SharedFile", "ObjectSegments"])
        self.assertEqual(plan["build_commands"], [])
        self.assertEqual(set(plan["planned_compose_commands"]), set(runner.BACKENDS))
        self.assertTrue(all(project.startswith("eaccept-eaoffline-") for project in plan["projects"].values()))
        self.assertIn("INCOMPLETE", (ROOT / "scripts/acceptance/helm-lifecycle.prerequisites.json").read_text())

    def test_execute_example_fails_before_docker_build_or_provisioning(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            args = self.args(Path(directory))
            manifest = json.loads(args.manifest.read_text())
            with mock.patch.object(runner.subprocess, "Popen") as process:
                with self.assertRaises(runner.AcceptanceError):
                    runner.execute(args, manifest)
            process.assert_not_called()
            self.assertFalse(args.work_root.exists())

    def test_two_job_build_requires_explicit_central_permission(self):
        with self.assertRaisesRegex(runner.AcceptanceError, "permit-build"):
            runner.cli(["--build-tools"])
        args = runner.cli(["--build-tools", "--permit-build", "--run-id", "eaoffline"])
        command = runner.plan(args, {})["build_commands"][0]
        self.assertEqual(command[command.index("-j") + 1], "2")
        self.assertIn("--target-dir", command)

    def test_missing_required_storage_profile_is_not_a_skip(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            with self.assertRaisesRegex(runner.AcceptanceError, "Both SharedFile and ObjectSegments"):
                runner.validate(self.args(Path(directory)), {"schema_version": 1, "docker_context": "default", "profiles": {"SharedFile": {}}})

    def test_rejected_upgrade_cannot_change_any_persisted_pin_or_override(self):
        original = {"core": {"release_id": 1, "policy_revision": 2, "digest": "measured-fixture"}, "config": {"max_action_timeout": 77}}
        runner.check_snapshot(original, original)
        for changed in ({"core": dict(original["core"], release_id=3), "config": original["config"]}, {"core": dict(original["core"], policy_revision=3), "config": original["config"]}, {"core": original["core"], "config": {"max_action_timeout": 600}}):
            with self.assertRaisesRegex(runner.AcceptanceError, "mutated"):
                runner.check_snapshot(changed, original)

    def test_foreign_archive_and_wrong_measured_digest_are_rejected(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            archive = root / "actual-test-bytes.tar.gz"
            archive.write_bytes(b"unit-test archive boundary, not a release artifact")
            response = {"archive_path": str(archive), "digest": runner.digest(archive)}
            with mock.patch.object(runner, "command", return_value=json.dumps(response)):
                with self.assertRaisesRegex(runner.AcceptanceError, "foreign"):
                    runner.archive(Path("not-executed"), root, root / "other")
            response["digest"] = "0" * 64
            with mock.patch.object(runner, "command", return_value=json.dumps(response)):
                with self.assertRaisesRegex(runner.AcceptanceError, "digest mismatch"):
                    runner.archive(Path("not-executed"), root, root)

    def stack(self, root):
        args = self.args(root)
        args.work_root.mkdir()
        profile = {"compose_file": str(root / "fixture.yaml"), "foundation": ["postgres"], "writers": ["api", "worker"], "initializers": [], "migrations": "migrations", "action_host": "worker", "sensor_host": "worker"}
        stack = runner.Stack(args, {"source_revision": "a" * 40, "docker_context": "default"}, "SharedFile", profile)
        return stack

    def test_foreign_resource_is_never_stopped_or_removed(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            stack = self.stack(Path(directory))
            stack.created = True
            foreign = {"Config": {"Labels": {runner.OWNER: "someone-else", "com.docker.compose.project": stack.project}}, "State": {"Running": True}}
            with mock.patch.object(stack, "ids", return_value=["foreign-id"]), mock.patch.object(stack, "docker", return_value=json.dumps([foreign])) as docker:
                with self.assertRaisesRegex(runner.AcceptanceError, "foreign"):
                    stack.cleanup()
            self.assertEqual(docker.call_args.args[:2], ("container", "inspect"))
            self.assertEqual(docker.call_count, 1)

    def test_cleanup_joins_writers_before_removing_exact_verified_volume(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            stack = self.stack(Path(directory))
            stack.root.mkdir()
            stack.file.write_text("{}")
            stack.created = True
            events = []
            volumes = ["only-owned-volume"]

            def ids(kind, owned_only=True):
                return list(volumes) if kind == "volume" else []

            def docker(*args):
                if args[:2] == ("volume", "rm"):
                    self.assertEqual(args[2:], ("only-owned-volume",))
                    events.append("remove-volume")
                    volumes.clear()
                return ""

            with mock.patch.object(stack, "stop_writers", side_effect=lambda: events.append("writers-joined")), mock.patch.object(stack, "ids", side_effect=ids), mock.patch.object(stack, "assert_owned", side_effect=lambda *args: events.append("verified-owner")), mock.patch.object(stack, "compose", side_effect=lambda *args: events.append("down-without-volumes")), mock.patch.object(stack, "docker", side_effect=docker):
                stack.cleanup()
            self.assertLess(events.index("writers-joined"), events.index("down-without-volumes"))
            self.assertLess(events.index("down-without-volumes"), events.index("remove-volume"))
            self.assertGreaterEqual(events.count("verified-owner"), 2)
            self.assertFalse(stack.file.exists())

    def test_unjoined_fixture_group_preserves_database_volumes(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            stack = self.stack(Path(directory))
            stack.created = True
            runner.ACTIVE_GROUPS.add(99999999)
            try:
                with mock.patch.object(stack, "stop_writers"), mock.patch.object(stack, "compose") as compose:
                    with self.assertRaisesRegex(runner.AcceptanceError, "not joined"):
                        stack.cleanup()
                    compose.assert_not_called()
            finally:
                runner.ACTIVE_GROUPS.clear()

    def test_generated_model_removes_dev_names_ports_and_scopes_credentials(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            stack = self.stack(Path(directory))
            stack.profile["writers"].append("web")
            stack.profile["old_image"] = "fixture-old@sha256:" + "a" * 64
            names = ["postgres", "api", "worker", "web", "migrations"]
            model = {"services": {name: {"container_name": "normal-dev-" + name, "image": "not-used", "ports": ["8080:8080"], "environment": {}, "volumes": ["data:/data"]} for name in names}, "volumes": {"data": {"name": "normal-dev-data"}}, "networks": {"attune-network": {"name": "normal-dev-network", "ipam": {"config": [{"subnet": "172.28.0.0/16"}]}}}}
            model["services"]["api"]["volumes"] += ["data:/opt/attune/blobs", "data:/opt/attune/artifacts"]
            model["services"]["worker"]["volumes"] += ["data:/opt/attune/artifacts"]
            Path(stack.profile["compose_file"]).write_text(json.dumps(model))
            stack.profile["images"] = {name: ("postgres:18-alpine" if name == "postgres" else "fixture-" + name) + "@sha256:" + "a" * 64 for name in names}

            def docker(*args):
                return json.dumps([{"Config": {"Labels": {"org.opencontainers.image.revision": "a" * 40}}}]) if args[:2] == ("image", "inspect") else ""

            with mock.patch.object(stack, "docker", side_effect=docker):
                stack.prepare()
            isolated = json.loads(stack.file.read_text())
            self.assertNotIn("name", isolated["volumes"]["data"])
            self.assertNotIn("ipam", isolated["networks"]["attune-network"])
            for name, service in isolated["services"].items():
                self.assertNotIn("container_name", service)
                self.assertEqual(service["labels"], {runner.OWNER: "eaoffline"})
                if name in ("api", "postgres", "web"):
                    self.assertEqual(service["ports"][0]["published"], "0")
                    self.assertEqual(service["ports"][0]["host_ip"], "127.0.0.1")
                else:
                    self.assertNotIn("ports", service)
            for name in ("worker", "web"):
                self.assertNotIn("DB_PASSWORD", isolated["services"][name]["environment"])
                self.assertNotIn("ATTUNE__DATABASE__URL", isolated["services"][name]["environment"])

    def test_unconverted_readiness_fails_before_credential_provisioning(self):
        spec = importlib.util.spec_from_file_location("actual_publisher_guard", ROOT / "scripts/bootstrap_core_pack.py")
        publisher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(publisher)
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            stack = self.stack(Path(directory))
            with mock.patch.object(stack, "up"), mock.patch.object(stack, "compose", return_value="180001"), mock.patch.object(stack, "endpoint", return_value="127.0.0.1:12345"), mock.patch.object(runner, "http", return_value={"bootstrap_admission_protocol": None}), mock.patch.object(runner, "fixture") as fixture:
                with self.assertRaisesRegex(runner.AcceptanceError, "Unconverted"):
                    runner.lifecycle(stack, Path("not-executed"), ROOT / "packs/core", {}, publisher)
            fixture.assert_not_called()

    def test_zero_or_ignored_f_case_is_not_acceptance(self):
        inventory = "\n".join(name + ": test" for name in runner.F_CASES)
        for outcome in ("0 passed; 0 failed; 0 ignored;", "0 passed; 0 failed; 1 ignored;"):
            with mock.patch.object(runner, "command", side_effect=[inventory, outcome]):
                with self.assertRaisesRegex(runner.AcceptanceError, "exactly once"):
                    runner.required_f_cases("not-executed", {})

    def test_foreground_command_is_joined_and_inherited_secrets_are_not_forwarded(self):
        process = mock.Mock(pid=99999999, returncode=0)
        process.communicate.return_value = ("result", "")

        def killpg(pid, sig):
            if sig == 0:
                raise ProcessLookupError

        with mock.patch.dict(runner.os.environ, {"PATH": "/usr/bin", "ATTUNE_BOOTSTRAP_TOKEN": "not-a-live-secret"}, clear=True), mock.patch.object(runner.subprocess, "Popen", return_value=process) as popen, mock.patch.object(runner.os, "killpg", side_effect=killpg):
            self.assertEqual(runner.command(["not-executed"]), "result")
        process.wait.assert_called_once()
        self.assertTrue(popen.call_args.kwargs["start_new_session"])
        self.assertNotIn("ATTUNE_BOOTSTRAP_TOKEN", popen.call_args.kwargs["env"])
        self.assertFalse(runner.ACTIVE_GROUPS)

    def test_no_account_backdoor_reset_or_gate_flip_and_exact_f_contract(self):
        source = (ROOT / "scripts/run-bootstrap-lifecycle-acceptance.py").read_text()
        for forbidden in ("CREATE USER", "INSERT INTO identity", "DROP SCHEMA", "docker system prune", "volume prune", "--volumes", "BOOTSTRAP_ADMISSION_ENABLED=true"):
            # The safety comment deliberately mentions the forbidden down flag.
            if forbidden != "--volumes":
                self.assertNotIn(forbidden, source)
        self.assertIn("stack.stop_writers()  # F preflight", source)
        self.assertIn('"--exact", "--test-threads=4"', source)
        self.assertIn("1 passed; 0 failed; 0 ignored;", source)
        self.assertIn("exact_event_pin_blocks_leaf_drop_but_not_bounded_unpinned_row_cleanup", runner.F_CASES)
        helper = (ROOT / "crates/common/examples/bootstrap_acceptance_archive.rs").read_text()
        self.assertIn("PackStorage::new(output).publish_release", helper)
        self.assertIn("linux-amd64", helper)
        self.assertIn("linux-arm64", helper)


if __name__ == "__main__":
    unittest.main()
