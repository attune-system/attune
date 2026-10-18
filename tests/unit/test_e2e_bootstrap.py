"""Offline coverage for owned E2E publication credentials."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).parents[1] / "e2e/configure_bootstrap.py"
SPEC = importlib.util.spec_from_file_location("e2e_bootstrap", SCRIPT)
bootstrap = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bootstrap)
TOKEN = "test-owned-publication-credential"
ENVIRONMENT = {
    "ATTUNE_API_URL": "http://api:8080",
    "TEST_USER_LOGIN": "owned@example.test",
    "TEST_USER_PASSWORD": "owned-password",
    "ATTUNE_E2E_RUN_ID": "bootstrap-test",
}


class Response:
    def __init__(self, data, status=200):
        self.status_code = status
        self.payload = {"data": data}

    def json(self):
        return self.payload


class Session:
    def __init__(self):
        self.headers = {}
        self.calls = []
        self.closed = False
        self.login_status = 200
        self.delete_status = 200
        self.revoked_status = 401

    def post(self, url, **options):
        self.calls.append(("POST", url, options))
        if url.endswith("/auth/login"):
            return Response({"access_token": "test-login-token"}, self.login_status)
        if url.endswith("/auth/token-login"):
            if options.get("json") != {"token": TOKEN}:
                raise AssertionError("Probe must use the raw integration credential contract")
            return Response({}, self.revoked_status)
        return Response({"token": TOKEN, "integration_token": {"id": 9}}, 201)

    def get(self, url, **options):
        self.calls.append(("GET", url, options))
        if "headers" in options:
            raise AssertionError("Do not probe a raw integration credential as a JWT")
        return Response({"id": 7})

    def delete(self, url, **options):
        self.calls.append(("DELETE", url, options))
        return Response({}, self.delete_status)

    def close(self):
        self.closed = True


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = Path(self.directory.name)
        self.session = Session()
        self.addCleanup(self.directory.cleanup)

    def configure(self, operation):
        bootstrap.configure(operation, self.path, ENVIRONMENT, lambda: self.session)

    def test_issue_uses_authenticated_api_and_private_files_without_output(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            self.configure("issue")
        self.assertNotIn(TOKEN, output.getvalue())
        self.assertEqual((self.path / "token").read_text(), TOKEN + "\n")
        self.assertEqual((self.path / "token").stat().st_mode & 0o777, 0o600)
        self.assertEqual((self.path / "credential.json").stat().st_mode & 0o777, 0o600)
        record = json.loads((self.path / "credential.json").read_text())
        self.assertEqual(record, {"api_origin": "http://api:8080", "identity": 7, "token_id": 9})
        self.assertNotIn(TOKEN, json.dumps(record))
        creation = self.session.calls[-1]
        self.assertTrue(creation[1].endswith("/identities/7/integration-tokens"))
        self.assertEqual(creation[2]["timeout"], 20)
        self.assertEqual(self.session.headers["Authorization"], "Bearer test-login-token")
        self.assertTrue(self.session.closed)

    def test_repeated_issue_reuses_owned_files_without_minting_another_token(self):
        self.configure("issue")
        self.configure("issue")
        creations = [call for call in self.session.calls if call[1].endswith("/integration-tokens")]
        self.assertEqual(len(creations), 1)

    def test_revoke_uses_recorded_owner_and_token_id_and_removes_secret_file(self):
        self.configure("issue")
        self.configure("revoke")
        self.assertFalse((self.path / "token").exists())
        self.assertTrue((self.path / "credential.json").exists())
        self.assertEqual(self.session.calls[-2][0:2],
                         ("DELETE", "http://api:8080/api/v1/identities/7/integration-tokens/9"))
        self.assertEqual(self.session.calls[-1][0:2], ("POST", "http://api:8080/auth/token-login"))
        self.assertEqual(self.session.calls[-1][2]["json"], {"token": TOKEN})
        self.session.delete_status = 404
        self.configure("revoke")

    def test_revoke_without_an_owned_record_does_not_contact_api(self):
        self.configure("revoke")
        self.assertEqual(self.session.calls, [])

    def test_revoke_fails_if_the_issued_credential_remains_accepted(self):
        self.configure("issue")
        self.session.revoked_status = 200
        with self.assertRaisesRegex(RuntimeError, "still accepted"):
            self.configure("revoke")
        self.assertTrue((self.path / "token").exists())

    def test_revoke_does_not_accept_descriptor_from_another_api(self):
        self.configure("issue")
        record = self.path / "credential.json"
        record.write_text(json.dumps({"api_origin": "http://other", "identity": 7, "token_id": 9}))
        with self.assertRaisesRegex(RuntimeError, "another API or identity"):
            self.configure("revoke")
        self.assertFalse(any(call[0] == "DELETE" for call in self.session.calls))

    def test_insecure_file_mode_and_symlink_are_rejected(self):
        self.configure("issue")
        token = self.path / "token"
        token.chmod(0o644)
        with self.assertRaisesRegex(RuntimeError, "0600"):
            self.configure("issue")
        token.unlink()
        token.symlink_to(self.path / "credential.json")
        with self.assertRaises(OSError):
            self.configure("issue")

    def test_secret_write_failure_revokes_the_new_token(self):
        original = bootstrap.private_write

        def fail_token(path, value):
            if path.name == "token":
                raise OSError("simulated write failure")
            original(path, value)

        with patch.object(bootstrap, "private_write", side_effect=fail_token):
            with self.assertRaises(OSError):
                self.configure("issue")
        self.assertEqual(self.session.calls[-1][0:2],
                         ("DELETE", "http://api:8080/api/v1/identities/7/integration-tokens/9"))

    def test_failed_login_does_not_create_files_or_emit_response_body(self):
        self.session.login_status = 401
        with self.assertRaisesRegex(RuntimeError, "HTTP 401"):
            self.configure("issue")
        self.assertEqual(list(self.path.iterdir()), [])
        self.assertTrue(self.session.closed)

    def test_main_sanitizes_errors_and_never_prints_credential_material(self):
        output = io.StringIO()
        with patch.object(bootstrap, "configure", side_effect=RuntimeError(TOKEN)), \
             patch.object(bootstrap.sys, "argv", ["configure_bootstrap.py", "issue", "--directory", str(self.path)]), \
             contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            self.assertEqual(bootstrap.main(), 1)
        self.assertNotIn(TOKEN, output.getvalue())


class RunnerCleanupTests(unittest.TestCase):
    def test_failed_revocation_retains_only_descriptor_and_returns_failure(self):
        for teardown in ("true", "false"):
            with self.subTest(teardown=teardown):
                self.check_cleanup(revoke_code=1, teardown=teardown)

    def test_successful_revocation_removes_private_directory(self):
        self.check_cleanup(revoke_code=0, teardown="false")

    def test_original_failure_survives_failed_revocation(self):
        self.check_cleanup(revoke_code=1, teardown="false", original_code=143)

    def check_cleanup(self, revoke_code, teardown, original_code=0):
        runner = SCRIPT.parents[2] / "scripts/run-integration-tests.sh"
        contents = runner.read_text()
        cleanup = contents[contents.index("cleanup() {\n"):contents.index("\ntrap cleanup EXIT")]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            private = root / "bootstrap"
            private.mkdir(mode=0o700)
            (private / "token").write_text(TOKEN)
            descriptor = private / "credential.json"
            descriptor.write_text('{"identity":7,"token_id":9}')
            descriptor.chmod(0o600)
            run_root = root / "run"
            run_root.mkdir()
            environment = dict(os.environ, BOOTSTRAP_ROOT=str(private),
                               RUN_ROOT=str(run_root), BOOTSTRAP_OWNED="true",
                               DO_TEARDOWN=teardown, DO_STARTUP="true",
                               STACK_STARTED="true", COMPOSE_PROJECT_NAME="owned-test")
            script = f"""set -euo pipefail
bootstrap_credential() {{ return {revoke_code}; }}
compose() {{ return 0; }}
log_error() {{ printf '%s\\n' "$*"; }}
log_info() {{ printf '%s\\n' "$*"; }}
{cleanup}
set +e
(exit {original_code})
cleanup
"""
            result = subprocess.run(["bash", "-c", script], env=environment,
                                    text=True, capture_output=True, timeout=10)
            self.assertEqual(result.returncode, original_code or revoke_code)
            self.assertNotIn(TOKEN, result.stdout + result.stderr)
            self.assertFalse((private / "token").exists())
            self.assertFalse(run_root.exists())
            if revoke_code:
                self.assertEqual(json.loads(descriptor.read_text()), {"identity": 7, "token_id": 9})
                self.assertEqual(descriptor.stat().st_mode & 0o777, 0o600)
                self.assertIn(str(descriptor), result.stdout)
            else:
                self.assertFalse(private.exists())


if __name__ == "__main__":
    unittest.main()
