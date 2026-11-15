"""Offline public-route/provenance tests. No process, server or DB is started."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent / "acceptance"))
import provision_operator as fixture
from fixture_support import FixtureError, read_secret, write_private


class RouteModel:
    def __init__(self):
        self.users = {1: {"id": 1, "login": "operator", "frozen": False, "password": "unit-test-only"}}
        self.grants, self.refs, self.tokens = {}, {}, {}
        self.access_scope = {}
        self.paths = []

    def call(self, path, token=None, body=None, method="GET", expected=(200, 201)):
        self.paths.append((method, path))
        if path == "/auth/login":
            if set(body) != {"login", "password"}:
                raise FixtureError("Only login/password belong in canonical login requests")
            identity = next(row for row in self.users.values() if row["login"] == body["login"])
            if identity.get("password") != body["password"]:
                raise FixtureError("Invalid local login/password")
            self.access_scope[identity["id"]] = None
            return {"data": {"access_token": str(identity["id"])}}
        if path == "/auth/token-login":
            identity = self.tokens[body["token"]]
            self.access_scope[identity] = "integration_token"
            return {"data": {"access_token": str(identity)}}
        if path == "/auth/me":
            identity = int(token)
            if self.users[identity]["frozen"]:
                if 403 not in expected:
                    raise FixtureError("Frozen identity unexpectedly used")
                return {"denied_status": 403}
            return {"data": {"id": identity, "effective_permissions": self.grants.get(identity, []), "assigned_permission_set_refs": self.refs.get(identity, []), "auth_provider": "local", "is_local": True, "can_change_password": bool(self.users[identity].get("password"))}}
        if path == "/api/v1/identities":
            identity = len(self.users) + 1
            self.users[identity] = dict(body, id=identity, frozen=False)
            return {"data": self.users[identity]}
        if path == "/api/v1/permissions/sets":
            self.grants[body["ref"]] = body["grants"]
            return {"data": body}
        if path == "/api/v1/permissions/assignments":
            self.grants[body["identity_id"]] = self.grants[body["permission_set_ref"]]
            self.refs[body["identity_id"]] = [body["permission_set_ref"]]
            return {"data": body}
        if path == "/api/v1/retention-config/file-audits/capabilities":
            self.audit_token = token
            if self.access_scope.get(int(token)) is not None:
                raise FixtureError("Audit requires unscoped Access")
            return {"data": {"scope": "unit-test-only"}}
        identity = int(path.split("/")[4])
        if path.endswith("/freeze"):
            self.users[identity]["frozen"] = True
            return {"data": {}}
        if path.endswith("/integration-tokens"):
            secret = "unit-test-only-token-" + str(identity)
            self.tokens[secret] = identity
            return {"data": {"token": secret, "integration_token": {"id": identity + 10, "created_by": int(token), "identity_id": identity, "active": True}}}
        return {"data": self.users[identity]}


class OperatorTests(unittest.TestCase):
    def test_public_provisioning_uses_distinct_scoped_identities_and_frozen_issuer(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            human = root / "human.private"
            write_private(human, {"kind": "password", "login": "operator", "password": "unit-test-only", "owned_project": "eaccept-test-sharedfile"})
            context = {"api_url": "http://127.0.0.1:1234", "project": "eaccept-test-sharedfile", "run_id": "test", "compose_file": str(root / "compose.private.json"), "operator_credential_file": str(human), "credential_output": str(root / "installer.private"), "invoker_credential_output": str(root / "invoker.private")}
            api = RouteModel()
            result = fixture.provision(context, api)
            self.assertTrue(result["ordinary_operator_provisioning"])
            self.assertTrue(result["issuer_frozen"])
            self.assertFalse(api.users[1]["frozen"])
            self.assertEqual(api.grants[3], fixture.INSTALLER)
            self.assertEqual(api.grants[4], fixture.INVOKER)
            self.assertNotEqual(read_secret(context["credential_output"]), read_secret(context["invoker_credential_output"]))
            self.assertNotIn("unit-test-only-token", json.dumps(result))
            self.assertFalse(any("roles" in path for _, path in api.paths))
            self.assertEqual(human.read_text(), json.dumps({"kind": "password", "login": "operator", "password": "unit-test-only", "owned_project": "eaccept-test-sharedfile"}))

    def test_secret_file_refuses_world_readable_and_symlink(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            path = Path(directory) / "private"
            path.write_text("not-live")
            path.chmod(0o644)
            with self.assertRaises(FixtureError):
                read_secret(path)
            link = path.with_name("symlink")
            link.symlink_to(path)
            with self.assertRaises(OSError):
                read_secret(link)

    def test_onboarding_denial_emits_no_credentials_or_first_admin_fallback(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            human = root / "human.private"
            write_private(human, {"kind": "access_token", "access_token": "unit-test-only", "owned_project": "eaccept-test-sharedfile"})
            ctx = {"api_url": "http://127.0.0.1:1234", "project": "eaccept-test-sharedfile", "run_id": "test", "compose_file": str(root / "compose.private.json"), "operator_credential_file": str(human), "credential_output": str(root / "installer.private"), "invoker_credential_output": str(root / "invoker.private")}
            class DeniedAPI:
                def call(self, *args, **kwargs):
                    raise FixtureError("ordinary operator authorization denied")
            with self.assertRaisesRegex(FixtureError, "operator authorization"):
                fixture.provision(ctx, DeniedAPI())
            self.assertFalse(Path(ctx["credential_output"]).exists())
            self.assertFalse(Path(ctx["invoker_credential_output"]).exists())

    def test_extra_inherited_grants_fail_closed(self):
        api = RouteModel()
        api.grants[1] = [{"resource": "permissions", "actions": ["manage"]}]
        api.refs[1] = ["test.installer"]
        with self.assertRaisesRegex(FixtureError, "least-scoped"):
            fixture.verify_me(api, "1", 1, fixture.INSTALLER, "test.installer")

    def test_no_first_admin_or_human_credential_in_pack_invoker_grants(self):
        source = Path(fixture.__file__).read_text()
        self.assertNotIn("/auth/register", source)
        self.assertNotIn("core.admin", source)
        self.assertEqual({grant["resource"] for grant in fixture.INSTALLER}, {"packs"})
        self.assertNotIn("permissions", {grant["resource"] for grant in fixture.INVOKER})

    def storage_context(self, root):
        human = root / "human.private"
        write_private(human, {"kind": "password", "login": "operator", "password": "unit-test-only", "owned_project": "eaccept-test-sharedfile"})
        return {"api_url": "http://127.0.0.1:1234", "project": "eaccept-test-sharedfile", "run_id": "test", "compose_file": str(root / "compose.private.json"), "operator_credential_file": str(human), "credential_output": str(root / "installer.private"), "invoker_credential_output": str(root / "invoker.private"), "maintenance_credential_output": str(root / "maintenance.private"), "storage_fixture": {"allow_shell_invocation": True, "allow_maintenance_management": True, "legacy_artifact_ref": "core.real_legacy_test", "legacy_owner_ref": "core.run_agent_command"}}

    def test_retired_storage_fixture_fails_before_api_or_credential_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            ctx = self.storage_context(Path(directory))
            api = RouteModel()
            with self.assertRaisesRegex(Exception, "Deferred coverage.*142"):
                fixture.provision(ctx, api)
            for field in ("credential_output", "invoker_credential_output", "maintenance_credential_output"):
                self.assertFalse(Path(ctx[field]).exists())


if __name__ == "__main__":
    unittest.main()
