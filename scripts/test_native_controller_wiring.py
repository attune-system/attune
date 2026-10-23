"""Offline E packaging/protocol checks; no systemd, kernel, DB or service jobs."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts/acceptance"))
SPEC = importlib.util.spec_from_file_location("e_native_lifecycle", ROOT / "scripts/acceptance/native_controller_lifecycle.py")
native = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(native)


class NativeWiring(unittest.TestCase):
    def test_default_plan_does_not_read_binding_or_spawn(self):
        with mock.patch.object(sys, "argv", ["native", "--binding", "/does-not-exist"]), mock.patch.object(native, "read_secret") as read, contextlib.redirect_stdout(io.StringIO()) as output:
            native.main()
        read.assert_not_called()
        result = json.loads(output.getvalue())
        self.assertEqual(result["mode"], "DRY_RUN")
        self.assertFalse(result["mutations"])
        self.assertFalse(result["full_sow_acceptance"])

    def test_explicit_execute_without_second_flag_never_reads_credentials(self):
        with mock.patch.object(sys, "argv", ["native", "--binding", "/absent", "--execute"]), mock.patch.object(native, "read_secret") as read:
            with self.assertRaisesRegex(native.FixtureError, "both explicit"):
                native.main()
        read.assert_not_called()

    def test_disabled_binding_rejected_before_artifact_or_service_access(self):
        with mock.patch.object(native, "root_file") as read:
            with self.assertRaises(native.FixtureError):
                native.validate({"enabled_for_controlled_acceptance": False})
        read.assert_not_called()

    def test_all_three_modes_required_before_setup(self):
        with mock.patch.object(native, "root_file") as read:
            with self.assertRaisesRegex(native.FixtureError, "All three"):
                native.validate({"enabled_for_controlled_acceptance": True, "run_id": "ea-test", "cases": [{"mode": "blob"}, {"mode": "shared"}]})
        read.assert_not_called()

    def test_foreign_units_cannot_be_controlled(self):
        command = mock.Mock()
        owner = native.Lifecycle({}, SimpleNamespace(command=command))
        for unit in ("sshd.service", "attune-native-legacy@../other.service", "/tmp/attune-native-legacy@x.service"):
            with self.assertRaises(native.FixtureError):
                owner.systemd("stop", unit)
        command.assert_not_called()

    def test_receipts_are_paginated_and_stalled_cursor_fails(self):
        owner = native.Lifecycle({}, None)
        api = mock.Mock()
        api.call.side_effect = [{"data": [{"kind": "absence", "identity": "a"}]}, {"data": [{"kind": "writers", "identity": "b"}]}, {"data": []}]
        result = owner.receipts(api, "/audit/1", "offline-token")
        self.assertEqual([r["kind"] for r in result], ["absence", "writers"])
        self.assertIn("after_kind=absence", api.call.call_args_list[1].args[0])
        api.call.side_effect = [{"data": [{"kind": "writers", "identity": "b"}]}] * 2
        with self.assertRaisesRegex(native.FixtureError, "advance"):
            owner.receipts(api, "/audit/1", "offline-token")

    def test_package_keeps_writer_outside_daemon_and_requires_real_source(self):
        unit = (ROOT / "docker/acceptance/attune-native-legacy@.service").read_text()
        for value in ("User=61124", "Group=61124", "NoNewPrivileges=yes", "CapabilityBoundingSet=\n", "python3 -I -B", "Restart=no", "KillMode=process"):
            self.assertIn(value, unit)
        self.assertNotIn("[Install]", unit.split("# No [Install]")[0])
        self.assertNotIn("Delegate=yes", unit)
        recipe = (ROOT / "docker/Dockerfile.acceptance-artifacts").read_text()
        self.assertIn("--example legacy_native_writer", recipe)
        self.assertIn("for module in main controller journal native protocol", recipe)
        self.assertIn("protocol http_input", recipe)
        self.assertIn("--example native_settlement_adapter", recipe)
        self.assertEqual(set(native.SOURCE_PINS), set(native.MODULES))
        self.assertNotIn("docker.sock", unit)

    def test_positive_flow_has_real_api_and_repo_assertions_not_head_fallback(self):
        source = (ROOT / "scripts/acceptance/native_controller_lifecycle.py").read_text()
        for required in ('self.barrier(config, case["mode"])', 'audit["plan"]["members"] == [original]', 'self.systemd("restart", unit)', 'released["pins"] == 0', 'released["unresolved_operations"] == 0', '"writers", "catalogue", "absence", "settled"'):
            self.assertIn(required, source)
        self.assertNotIn('method="HEAD"', source)
        self.assertNotIn("INSERT INTO", source)
        self.assertNotIn("rmtree", source)

    def test_real_adapter_request_contains_generation_and_no_caller_member(self):
        command = mock.Mock(return_value=json.dumps({"protocol": 1, "data": {"member": "offline response"}}))
        owner = native.Lifecycle({"run_id": "owned", "producer_adapter": {"binary": "/owned/native_settlement_adapter"}}, SimpleNamespace(command=command))
        case = {"mode": "blob", "project": "eaccept-owned-native-blob", "generation": "original-generation", "producer_context_file": "/owned/context.private.json"}
        result = owner.inspect(case, "prepare-original-native-producer")
        request = json.loads(command.call_args.kwargs["data"])
        self.assertEqual(request["generation"], case["generation"])
        self.assertNotIn("member", request)
        self.assertEqual(command.call_args.kwargs["env"], {"ATTUNE_NATIVE_ACCEPTANCE_CONTEXT": case["producer_context_file"]})
        self.assertEqual(result, {"member": "offline response"})

    def test_original_requires_actual_controlled_ids_and_both_obligations(self):
        principal = {"native_instance": "00000000-0000-4000-8000-000000000001", "worker_instance": "00000000-0000-4000-8000-000000000002", "execution_attempt": "00000000-0000-4000-8000-000000000003"}
        member = dict(principal, operation="00000000-0000-4000-8000-000000000004", version=10, root_read=11, object_key="object", staging_path=".attune-upload/stage", provider_version=None)
        prepared = {"protocol": "controlled_native_v1", "scope": {"backend": "offline"}, "member": member, **{key: 1 for key in ("artifact_id", "artifact_version", "execution_id", "admission_id", "worker_id", "source_receipt")}}
        config = {"scope": prepared["scope"]}
        self.assertIs(native.producer.original(prepared, config, principal, "blob_and_shared"), member)
        for change in ({"protocol": "legacy_unknown"}, {"admission_id": None}, {"member": dict(member, staging_path=None)}, {"member": dict(member, native_instance="foreign")}):
            with self.assertRaises(native.FixtureError):
                native.producer.original(dict(prepared, **change), config, principal, "blob_and_shared")

    def test_release_requires_real_matching_completed_audit_and_signed_receipts(self):
        member, scope = {"operation": "original"}, {"backend": "original"}
        before = {"member": member, "scope": scope, "pins": 2, "unresolved_operations": 1, "evidence_owner_count": 1}
        native.producer.observation(before, member, scope)
        audit = {"state": "completed", "plan_matches_original": True, "plan": {"members": [member], "scope": scope}, "receipt_counts": {key: 1 for key in ("writers", "catalogue", "absence", "settled", "controller")}, "controller_receipts": [{"identity": "actual repository identity"}]}
        after = dict(before, pins=0, unresolved_operations=0, evidence_owner_count=0, audit=audit)
        native.producer.observation(after, member, scope, released=True)
        for changed in ({"pins": 1}, {"member": {}}, {"audit": dict(audit, plan_matches_original=False)}, {"audit": dict(audit, controller_receipts=[])}):
            with self.assertRaises(native.FixtureError):
                native.producer.observation(dict(after, **changed), member, scope, released=True)

    def test_private_context_uses_actual_login_not_template_or_caller_token(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            case = {"producer_context_file": str(root / "context.private.json")}
            template = {"access": {"maintenance_identity": 123}, "maintenance_access_token_file": str(root / "access.private")}
            with self.assertRaises(native.FixtureError):
                native.producer.context(case, template, "offline token", 999)
            self.assertFalse(Path(case["producer_context_file"]).exists())
            native.producer.context(case, template, "offline token", 123)
            self.assertEqual(json.loads(native.read_secret(case["producer_context_file"])), template)
            self.assertEqual(native.read_secret(template["maintenance_access_token_file"]), "offline token")

    def test_context_requires_exact_loopback_role_and_fresh_private_outputs(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            template_file = root / "template.private.json"
            config = {"generation": "generation", "issuer": "native", "audience": "audit", "roots": {"blob": "/owned/blob", "shared": "/owned/shared"}}
            case = {"run_id": "owned", "project": "eaccept-owned-native-blob", "mode": "blob", "producer_context_template_file": str(template_file), "producer_context_file": str(root / "producer.private.json"), "controller_endpoint": "https://127.0.0.1:9443"}
            template = {"enabled_for_controlled_acceptance": True, "database_url": "postgres://native_setup:fixture@127.0.0.1:1234/owned", "jwt_secret_file": str(root / "jwt.private"), "encryption_key_file": str(root / "encryption.private"), "maintenance_access_token_file": str(root / "access.private"), "access": {"run_id": case["run_id"], "project": case["project"], "generation": "generation", "mode": "blob", "producer_action_ref": "f_native_settlement.producer", "maintenance_identity": 10, "database_name": "owned", "database_role": "native_setup", "schema": "attune"}, "controller": {"endpoint": case["controller_endpoint"], "issuer": "native", "audience": "audit", "storage": {"provider": "filesystem", "root": "/owned/blob"}, "artifacts_dir": "/owned/shared", "verification_key_file": "/owned/signer", "client_identity_file": "/owned/manager", "ca_certificate_file": "/owned/ca"}}
            for path in (template["jwt_secret_file"], template["encryption_key_file"]):
                native.write_private(path, "offline-private-fixture")
            native.write_private(template_file, template)
            self.assertEqual(native.producer.validate_context(case, config, mock.Mock()), template)
            for url in ("postgres://native_setup:fixture@remote:1234/owned", "postgres://postgres:fixture@127.0.0.1:1234/owned", "postgres://native_setup:fixture@127.0.0.1:1234/foreign"):
                template_file.write_text(json.dumps(dict(template, database_url=url)))
                with self.assertRaisesRegex(native.FixtureError, "scoped role"):
                    native.producer.validate_context(case, config, mock.Mock())
            template_file.write_text(json.dumps(template))
            with self.assertRaisesRegex(native.FixtureError, "Caller members"):
                native.producer.validate_context(dict(case, member={}), config, mock.Mock())
            native.write_private(template["maintenance_access_token_file"], "old")
            with self.assertRaisesRegex(native.FixtureError, "never adopt"):
                native.producer.validate_context(case, config, mock.Mock())

    def test_managed_install_uses_digest_api_and_separate_installer(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            archive = Path(directory) / "fixture.tar.gz"
            archive.write_bytes(b"offline multipart transport fixture only")
            release = {"archive_file": str(archive), "digest": native.file_hash(archive)}
            api = mock.MagicMock(base="http://127.0.0.1:1234")
            response = mock.MagicMock(status=201)
            api.opener.open.return_value.__enter__.return_value = response
            api.call.return_value = {"data": {"ref": "f_native_settlement.producer", "id": 40}}
            with mock.patch.object(native.producer, "login", return_value=(api, "offline-token", 10)):
                self.assertEqual(native.producer.install({}, release, 20), 40)
            request = api.opener.open.call_args.args[0]
            self.assertEqual(request.full_url, "http://127.0.0.1:1234/api/v1/packs/upload")
            self.assertIn(release["digest"].encode(), request.data)
            self.assertNotIn(b"force", request.data)
            self.assertNotIn(b"skip", request.data)
            with mock.patch.object(native.producer, "login", return_value=(api, "offline-token", 20)):
                with self.assertRaisesRegex(native.FixtureError, "distinct"):
                    native.producer.install({}, release, 20)


if __name__ == "__main__":
    unittest.main()
