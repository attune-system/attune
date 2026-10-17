"""No services, Docker, DB or credentials. Fake transport tests only."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import shlex
import tempfile
import unittest
from unittest import mock

import storage_probe as probe
import storage_probe_producer as producer


class FakeAPI:
    def __init__(self):
        self.calls = []
        self.status = "running"
        self.deleted = False
        self.auth_calls = []

    def token_login(self, file):
        self.auth_calls.append(("token", str(file)))
        return str(file)

    def password_login(self, file, run_id, project):
        self.auth_calls.append(("password", str(file)))
        return str(file), {"id": 20, "login": "acceptance." + run_id.replace("-", "_") + ".maintenance"}

    def call(self, path, token=None, body=None, method="GET", expected=(200,), bytes_response=False, error_contains=None):
        self.calls.append((path, token, method, copy.deepcopy(body)))
        if path == "/auth/me":
            role = {"invoker": "invoker", "operator": "maintenance", "installer": "installer"}[Path(token).name]
            return {"data": {"id": {"invoker": 10, "operator": 20, "installer": 30}[Path(token).name],
                             "login": "acceptance.owned_run." + role}}
        if path.endswith("/file-audits/capabilities"):
            return {"data": {"management_transport": "manual_required"}}
        if path.endswith("/execute"):
            return {"data": {"id": 7}}
        if path.endswith("/cancel"):
            self.status = "cancelled"
            return {}
        if path == "/api/v1/executions/7":
            return {"data": {"id": 7, "status": self.status, "worker": 11, "executor": 10}}
        if path.endswith("/download"):
            if bytes_response:
                payload = b"F owned lifecycle owned-run\n" * 4096
                return {"http_status": 200, "size": len(payload), "sha256": hashlib.sha256(payload).hexdigest()}
            assert expected == (403,)
            assert error_contains == "File evidence readmission required"
            return {"http_status": 403}
        if method == "DELETE":
            self.deleted = True
            return {}
        if path.endswith("/file-audits"):
            return {"data": {"id": "12345678-1234-1234-1234-123456789abc"}}
        if path.endswith("/quarantine"):
            return {"data": {"state": "manual_required"}}
        if path.endswith("/file-evidence/91"):
            return {"data": {"unresolved_operations": 1, "object_done": False}}
        if path.endswith("/file-evidence/8"):
            return {"data": {"object_done": False}}
        raise AssertionError("unexpected API operation " + path)


class FakeNative:
    docker = ["fake-docker"]

    def __init__(self, api, backend):
        self.api, self.backend = api, backend
        self.restored = False
        self.commands = []
        self.fail = True
        self.pins_value = {"release_id": 1, "digest": "abc", "policy_revision": 2,
                           "admission": 3, "authority_digest": "frozen"}
        self.identity = {"device": 1, "inode": 2, "size": 30, "mtime_micros": 3, "sha256": "original"}

    def run(self, argv, **kwargs):
        self.commands.append(argv)
        if argv[-2] == "unpause":
            self.restored = True
        return ""

    def container(self, service):
        return {"Id": "a" * 64, "NetworkSettings": {"Ports": {"9000/tcp": [{"HostPort": "9900"}]}}}

    def writer_processes(self):
        return [["worker", 123]] if self.api.status == "running" else []

    def physical(self, root, key, operation="stat", expected=None):
        if operation == "swap":
            assert expected == self.identity
            return {"inode": 99}
        if operation == "restore":
            assert expected == {"original": self.identity, "replacement": {"inode": 99}}
            self.restored = True
            return self.identity
        if self.restored:
            return None
        return self.identity

    def inspect(self, kind, execution_id, identity=None, version_id=None):
        assert execution_id == 7
        if kind in ("protected", "ledger", "released"):
            assert identity == {"mock": "repository-observed-exact-file"}
        if kind == "pins":
            return self.pins_value
        if kind == "version":
            return {"id": 8, "artifact": 9, "version": 1, "execution": 7, "file_path": "stage/owned",
                    "staging_path": "stage/owned", "object_key": "artifacts/9/uploads/exact", "provider_version": "v:immutable" if self.backend == "ObjectSegments" else 'e:"2-3-1e"',
                    "body_state": "ready", "evidence_state": "retained", "object_done": False, "staging_done": False,
                    "_inspection_identity": {"mock": "repository-observed-exact-file"}}
        if kind == "protected":
            return {"body": True, "unsettled": False}
        if kind == "backend":
            return "shared_file" if self.backend == "SharedFile" else "object_segments"
        if kind == "ledger":
            if self.restored:
                return None
            return {"id": 12, "object_key": "artifacts/9/uploads/exact", "provider_version": "v:immutable" if self.backend == "ObjectSegments" else 'e:"2-3-1e"',
                    "attempts": 1 if self.api.deleted else 0, "failed": self.api.deleted and self.fail}
        if kind == "released":
            return {"owner": False, "pins": False, "operations": False}
        if kind == "legacy":
            assert version_id == 91
            return {"artifact": 92, "version": 1, "evidence_state": "quarantined"}
        raise AssertionError("unexpected typed inspection " + kind)


class FakeProvider:
    def __init__(self, native):
        self.native = native
        self.observed = []

    def call(self, operation):
        self.observed.append(operation)
        assert operation == "get-bucket-versioning"
        return {"Status": "Enabled"}

    def versions(self, key):
        self.observed.append(key)
        return {"versions": [] if self.native.restored else ["immutable"], "uploads": [], "delete_markers": []}


class ProbeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir="/tmp/opencode")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def make(self, backend="SharedFile"):
        context = {"phase": "begin", "project": "eaccept-owned-run-profile", "owner_label": "io.attune.acceptance.owner", "run_id": "owned-run", "backend": backend,
                   "api_url": "http://127.0.0.1:8080", "probe_state": str(self.root / "state"),
                   "credential_output": str(self.root / "installer"), "invoker_credential_output": str(self.root / "invoker"),
                   "expected_pins": {"fresh": {"release_id": 1, "digest": "abc", "policy_revision": 2}}}
        binding = {"project": "eaccept-owned-run-profile", "run_id": "owned-run", "backend": backend,
                   "allow_native_faults": True, "maintenance_credential_file": str(self.root / "operator"),
                   "python": "python3", "hold_seconds": 1800, "poll_seconds": 30,
                   "artifacts_root": "/owned/artifacts", "blob_root": "/owned/blobs", "provider_service": "rustfs",
                   "provider_endpoint": "http://127.0.0.1:9900", "provider_port": 9000, "legacy_version_id": 91}
        api = FakeAPI()
        native = FakeNative(api, backend)
        provider = FakeProvider(native)
        return probe.Probe(context, binding, api, native, provider), api, native, provider

    def password_file(self):
        path = self.root / "maintenance.private.json"
        value = {"kind": "password", "role": "maintenance", "run_id": "owned-run",
                 "owned_project": "eaccept-owned-run-profile", "identity_id": 20,
                 "login": "acceptance.owned_run.maintenance", "password": "mock-private-password"}
        path.write_text(json.dumps(value))
        path.chmod(0o600)
        return path, value

    def test_auth_uses_canonical_root_routes_and_single_protected_prefix(self):
        token_file = self.root / "invoker-token"
        token_file.write_text("attune_it_mock_private_token")
        token_file.chmod(0o600)
        password_file, record = self.password_file()
        for suffix in ("", "/", "/api/v1", "/api/v1/"):
            calls = []
            class Response(io.BytesIO):
                code = 200
            def opened(request, timeout):
                calls.append(request)
                if request.full_url.endswith("/auth/me"):
                    return Response(b'{"data":{"id":20,"login":"acceptance.owned_run.maintenance"}}')
                return Response(b'{"data":{"access_token":"mock-access"}}')
            with mock.patch.object(probe.urllib.request, "build_opener") as build:
                build.return_value.open.side_effect = opened
                api = probe.API("http://127.0.0.1:8080" + suffix)
                self.assertEqual(api.token_login(token_file), "mock-access")
                access, principal = api.password_login(password_file, "owned-run", "eaccept-owned-run-profile")
                self.assertEqual(principal, {"id": 20, "login": record["login"]})
                api.call("/auth/me", access)
                api.call("/api/v1/retention-config/file-audits/capabilities", access)
            self.assertEqual([r.full_url for r in calls], [
                "http://127.0.0.1:8080/auth/token-login", "http://127.0.0.1:8080/auth/login",
                "http://127.0.0.1:8080/auth/me", "http://127.0.0.1:8080/api/v1/retention-config/file-audits/capabilities"])
            self.assertEqual(json.loads(calls[0].data), {"token": "attune_it_mock_private_token"})
            self.assertEqual([request.get_method() for request in calls], ["POST", "POST", "GET", "GET"])
            self.assertEqual(json.loads(calls[1].data), {"login": record["login"], "password": record["password"]})
            self.assertIsNone(calls[1].get_header("Authorization"))
            self.assertEqual(calls[2].get_header("Authorization"), "Bearer mock-access")

    def test_password_and_token_kinds_cannot_be_interchanged_or_use_host_identity(self):
        path, record = self.password_file()
        api = probe.API("http://127.0.0.1:8080/api/v1")
        with mock.patch.object(api, "call") as call:
            with self.assertRaisesRegex(probe.ProbeError, "raw integration token"):
                api.token_login(path)
            for key, value in (("kind", "integration_token"), ("owned_project", "foreign"),
                               ("role", "installer"), ("login", "original-human")):
                invalid = dict(record, **{key: value})
                path.write_text(json.dumps(invalid))
                with self.assertRaises(probe.ProbeError):
                    api.password_login(path, "owned-run", "eaccept-owned-run-profile")
            call.assert_not_called()

    def test_noncanonical_auth_alias_and_foreign_base_are_rejected_without_request(self):
        api = probe.API("http://127.0.0.1:8080")
        with mock.patch.object(probe.urllib.request, "build_opener") as build:
            for path in ("/api/v1/auth/me", "/api/v1/auth/login", "/auth/unknown", "/api/v1/../auth/login"):
                with self.assertRaises(probe.ProbeError):
                    api.call(path)
            build.assert_not_called()
        for base in ("http://127.0.0.1:8080/other", "http://127.0.0.1:8080/api/v1?token=hidden", "http://example.test:8080"):
            with self.assertRaises(probe.ProbeError):
                probe.API(base)
        self.assertEqual(producer.api_origin("http://api:8080/api/v1/"), "http://api:8080")
        self.assertEqual(producer.api_origin("http://api:8080"), "http://api:8080")

    def test_scoped_access_capability_rejection_has_no_credential_fallback(self):
        # Same real HTTP rejection that an integration-scoped maintenance Access
        # receives from the owning operator gate. Do not reinterpret it as a pass.
        api = probe.API("http://127.0.0.1:8080")
        class Forbidden(io.BytesIO):
            code = 403
        with mock.patch.object(probe.urllib.request, "build_opener") as build:
            build.return_value.open.return_value = Forbidden(b'{"error":"operator scope rejected"}')
            with self.assertRaisesRegex(probe.ProbeError, "HTTP 403"):
                api.call("/api/v1/retention-config/file-audits/capabilities", "mock-scoped-access")
            self.assertEqual(build.return_value.open.call_count, 1)
        observed = []
        original = FakeAPI.call
        def reject(instance, path, *args, **kwargs):
            observed.append((instance, path))
            if path.endswith("/file-audits/capabilities"):
                raise probe.ProbeError("API operator scope rejected: HTTP 403")
            return original(instance, path, *args, **kwargs)
        with mock.patch.object(FakeAPI, "call", reject):
            with self.assertRaisesRegex(probe.ProbeError, "HTTP 403"):
                self.make()
        instance = observed[-1][0]
        self.assertEqual([kind for kind, _ in instance.auth_calls], ["token", "password", "token"])
        self.assertFalse(any(path.endswith("/execute") for _, path in observed))

    def test_auth_me_cannot_reuse_the_installer_as_maintenance(self):
        original = FakeAPI.call
        def duplicate(instance, path, token=None, **kwargs):
            response = original(instance, path, token, **kwargs)
            if path == "/auth/me" and Path(token).name == "installer":
                response["data"]["id"] = 20
            return response
        with mock.patch.object(FakeAPI, "call", duplicate):
            with self.assertRaisesRegex(probe.ProbeError, "distinct real identities"):
                self.make()

    def test_host_maintenance_path_is_rejected_before_read_or_login(self):
        p, _, _, _ = self.make()
        binding = dict(p.b, maintenance_credential_file=str(self.root.parent / "host-production.private"))
        api = FakeAPI()
        native = FakeNative(api, "SharedFile")
        with mock.patch.object(probe, "private") as read:
            with self.assertRaisesRegex(probe.ProbeError, "run's private directory"):
                probe.Probe(p.c, binding, api, native, FakeProvider(native))
            read.assert_not_called()
        self.assertEqual(api.auth_calls, [])

    def through_upgrade(self, backend="SharedFile"):
        p, api, native, provider = self.make(backend)
        self.assertEqual(p.begin(), {"backend": backend, "execution_id": 7})
        p.c["execution_id"] = 7
        p.c["expected_pins"]["upgrade"] = {"policy_revision": 99}
        self.assertTrue(p.upgraded()["pinned_execution_retained"])
        p.c["expected_pins"]["rollback"] = {"release_id": 1}
        return p, api, native, provider

    def test_both_backends_require_real_observation_chain_before_emitting_checks(self):
        for backend in ("SharedFile", "ObjectSegments"):
            with self.subTest(backend=backend):
                if (self.root / "state").exists():
                    (self.root / "state").unlink()
                p, api, native, provider = self.through_upgrade(backend)
                result = p.finish()
                self.assertTrue(all(result["checks"][k] for k in probe.CHECKS))
                self.assertTrue(native.restored)
                saved = json.loads(probe.private(self.root / "state"))
                self.assertEqual(saved["phase"], "finish")
                self.assertEqual(saved["observations"]["provider_failure"]["attempts"], 1)
                self.assertEqual(saved["observations"]["quarantine_denies_authority"]["http_status"], 403)
                dispatch = next(c for c in api.calls if c[0].endswith("/execute"))
                self.assertEqual(Path(dispatch[1]).name, "invoker")
                self.assertEqual(dispatch[3]["permission_set_refs"], ["standard"])
                self.assertIn("f-storage-probe-owned-run", dispatch[3]["config"]["command"])
                command = dispatch[3]["config"]["command"]
                self.assertNotIn("'", command, "core dotenv shim does not decode embedded single quotes")
                argv = shlex.split(command)
                self.assertEqual(argv[-3:], ["owned-run", "1800", "f-storage-probe-owned-run"])
                self.assertTrue(argv[2].startswith("import base64;exec(base64.b64decode(\""))
                self.assertEqual((self.root / "state").stat().st_mode & 0o777, 0o600)
                if backend == "ObjectSegments":
                    self.assertIn(["fake-docker", "pause", "a" * 64], native.commands)
                    self.assertIn(["fake-docker", "unpause", "a" * 64], native.commands)
                    self.assertIn("get-bucket-versioning", provider.observed)

    def test_pin_drift_denies_before_cleanup(self):
        p, api, native, _ = self.through_upgrade()
        native.pins_value = dict(native.pins_value, authority_digest="changed")
        with self.assertRaisesRegex(probe.ProbeError, "immutable execution pins"):
            p.finish()
        self.assertFalse(api.deleted)

    def test_wrong_backend_observation_cannot_be_overridden_by_binding_flags(self):
        p, _, native, _ = self.make()
        native.backend = "ObjectSegments"
        p.b["checks"] = {"backend_observed": True}
        with self.assertRaisesRegex(probe.ProbeError, "actual worker log backend"):
            p.begin()

    def test_unsettled_request_does_not_reach_cleanup(self):
        p, _, native, _ = self.make()
        original = native.inspect
        native.inspect = lambda kind, execution_id, **kwargs: {"body": True, "unsettled": True} if kind == "protected" else original(kind, execution_id, **kwargs)
        with self.assertRaisesRegex(probe.ProbeError, "unsettled operations"):
            p.begin()

    def test_finish_needs_actual_producer_marker_at_begin(self):
        p, _, native, _ = self.make()
        native.writer_processes = lambda: []
        with self.assertRaisesRegex(probe.ProbeError, "producer process marker"):
            p.begin()

    def test_filesystem_etag_must_match_actual_physical_generation(self):
        p, _, native, _ = self.make()
        native.identity["mtime_micros"] = 400
        with self.assertRaisesRegex(probe.ProbeError, "actual inode/mtime/size ETag"):
            p.begin()

    def test_missing_native_failure_cannot_be_replaced_with_flags(self):
        p, api, native, _ = self.through_upgrade()
        native.fail = False
        p.b["checks"] = {k: True for k in probe.CHECKS}
        def immediate(description, observe, timeout=None):
            result = observe()
            probe.require(result, "missing measured operation: " + description)
            return result
        p.wait = immediate
        with self.assertRaisesRegex(probe.ProbeError, "provider deletion failure"):
            p.finish()
        self.assertTrue(native.restored, "fault must be restored on failed observation")
        self.assertNotEqual(json.loads(probe.private(self.root / "state"))["phase"], "finish")

    def test_missing_legacy_fixture_is_explicit_no_unknown_settlement(self):
        p, _, _, _ = self.through_upgrade()
        p.b["legacy_version_id"] = None
        with self.assertRaisesRegex(probe.ProbeError, "public APIs cannot create legacy_unknown"):
            p.finish()
        state = json.loads(probe.private(self.root / "state"))
        self.assertIn("provider_cleanup_retry", state["observations"])
        self.assertNotIn("quarantine_denies_authority", state["observations"])

    def test_state_cannot_be_redispatched_or_finish_replayed(self):
        p, _, _, _ = self.through_upgrade()
        with self.assertRaisesRegex(probe.ProbeError, "do not redispatch"):
            p.begin()
        p.finish()
        with self.assertRaisesRegex(probe.ProbeError, "requires successful upgraded"):
            p.finish()

    def test_private_file_rejects_symlink_and_public_mode(self):
        original = self.root / "secret"
        original.write_text("test credential")
        original.chmod(0o644)
        with self.assertRaises(probe.ProbeError):
            probe.private(original)
        original.chmod(0o600)
        link = self.root / "link"
        link.symlink_to(original)
        with self.assertRaises(probe.ProbeError):
            probe.private(link)

    def test_ownership_rejects_unlabelled_service_before_exec_or_pause(self):
        c = {"docker_context": "owned", "owner_label": "f.owner", "run_id": "owned-run", "project": "owned_project"}
        calls = []
        def fake(argv, **kwargs):
            calls.append(argv)
            if "ps" in argv:
                return "a" * 12
            return json.dumps([{"Config": {"Labels": {}}, "Id": "a" * 64}])
        native = probe.Native(c, {}, fake)
        with self.assertRaisesRegex(probe.ProbeError, "ownership mismatch"):
            native.container("rustfs")
        self.assertFalse(any("exec" in call or "pause" in call for call in calls))

    def test_inspector_transport_is_pinned_typed_stdin_with_private_context_only(self):
        binary = self.root / "inspector"
        binary.write_bytes(b"mock precompiled inspector; never executed")
        binary.chmod(0o700)
        context_file = self.root / "inspector-context"
        context_file.write_text('{"private":"test context"}')
        context_file.chmod(0o600)
        binding = {"inspector_binary": str(binary), "inspector_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                   "inspector_context_file": str(context_file)}
        c = {"docker_context": "owned", "run_id": "owned-run", "project": "eaccept-owned-run-profile"}
        calls = []
        def fake(argv, **kwargs):
            calls.append((argv, kwargs))
            request = json.loads(kwargs["data"])
            self.assertEqual(request, {"scope": {"run_id": "owned-run", "project": "eaccept-owned-run-profile", "execution_id": 7},
                                      "operation": {"kind": "pins"}})
            self.assertEqual(kwargs["env"], {"ATTUNE_STORAGE_PROBE_INSPECTOR_CONTEXT": str(context_file)})
            self.assertEqual(argv, [str(binary), "--execute"])
            return json.dumps({"protocol": 1, "data": {"release_id": 1}, "identity": None})
        native = probe.Native(c, binding, fake)
        self.assertEqual(native.inspect("pins", 7), {"release_id": 1})
        self.assertEqual(len(calls), 1)
        binding["inspector_sha256"] = "0" * 64
        with self.assertRaisesRegex(probe.ProbeError, "artifact digest"):
            native.inspect("pins", 7)
        self.assertEqual(len(calls), 1, "mismatched artifact must never be invoked")
        with self.assertRaisesRegex(probe.ProbeError, "unsupported typed"):
            native.inspect("sql", 7)

    def test_source_has_no_python_db_bypass_or_schema_prefix(self):
        source = Path(probe.__file__).read_text()
        self.assertNotIn("SELECT ", source)
        self.assertNotIn("psql", source)
        self.assertNotIn("def sql(", source)
        repository = Path(__file__).resolve().parents[2] / "crates/common/src/repositories/storage_probe_inspection.rs"
        text = repository.read_text()
        self.assertNotIn("attune.", text)
        self.assertNotIn("SELECT *", text)
        self.assertIn("REPEATABLE READ READ ONLY", text)
        self.assertIn("access.verify(scope, &identity)", text)

    def test_s3_inventory_uses_exact_version_key_and_separate_private_environment(self):
        credential = self.root / "provider"
        credential.write_text(json.dumps({"project": "owned", "bucket": "owned-bucket", "endpoint": "http://127.0.0.1:9900",
            "region": "test", "access_key_id": "test-only-id", "secret_access_key": "test-only-secret"}))
        credential.chmod(0o600)
        calls = []
        def fake(argv, **kwargs):
            calls.append((argv, kwargs))
            if "list-object-versions" in argv:
                return json.dumps({"Versions": [{"Key": "prefix/exact", "VersionId": "actual"}, {"Key": "prefix/exact-other", "VersionId": "sibling"}]})
            return json.dumps({"Uploads": []})
        s3 = probe.S3({"project": "owned", "owned_bucket": "owned-bucket"}, {"provider_credentials_file": str(credential),
            "provider_endpoint": "http://127.0.0.1:9900", "object_prefix": "prefix", "aws": "/owned/aws"}, fake)
        self.assertEqual(s3.versions("exact"), {"versions": ["actual"], "delete_markers": [], "uploads": []})
        self.assertEqual(len(calls), 2)
        self.assertTrue(any("list-multipart-uploads" in argv for argv, _ in calls))
        for argv, kwargs in calls:
            self.assertNotIn("test-only-secret", str(argv))
            self.assertEqual(kwargs["env"]["AWS_SECRET_ACCESS_KEY"], "test-only-secret")
        s3.run = lambda *args, **kwargs: json.dumps({"IsTruncated": True})
        with self.assertRaisesRegex(probe.ProbeError, "inventory truncated"):
            s3.versions("exact")


if __name__ == "__main__":
    unittest.main()
