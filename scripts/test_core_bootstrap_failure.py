"""Real initializer -> publisher -> local mock API, without Docker or databases.

ELF header fixtures only exercise packaging validation, never native execution.
"""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("diagnostics", ROOT / "scripts/bootstrap_failure_diagnostics.py")
diagnostics = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(diagnostics)


class BootstrapFailureTests(unittest.TestCase):
    def test_only_structured_diagnostics_reach_ci(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            diagnostics.summarize([
                "Authorization: Bearer fixture-secret\n",
                "ATTUNE_BOOTSTRAP_DIAGNOSTIC token=fixture-secret\n",
                "ATTUNE_BOOTSTRAP_DIAGNOSTIC stage=publication error=bad secret\n",
                "ATTUNE_BOOTSTRAP_DIAGNOSTIC stage=publication outcome=failed exit=7\n",
            ])
        self.assertEqual(output.getvalue(), "ATTUNE_BOOTSTRAP_DIAGNOSTIC stage=publication outcome=failed exit=7\n")

    def test_runner_collects_before_revoke_and_teardown_preserving_failure(self):
        for failure in ("up", "wait"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                docker = root / "docker"
                docker.write_text("#!" + sys.executable + "\n" + r'''
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ["EVENTS"], "a") as f:
    f.write(json.dumps(args) + "\n")
if args[:2] in (["volume", "ls"], ["network", "ls"]):
    sys.exit(0)
if args[0] != "compose":
    sys.exit(99)
args = args[1:]
while args and args[0] in ("--project-name", "-f"):
    args = args[2:]
if args[:2] == ["ps", "-aq"]:
    sys.exit(0)
if args[0] == "run" and "configure_bootstrap.py" in " ".join(args):
    if "issue" in args:
        volume = args[args.index("--volume") + 1]
        pathlib.Path(volume.split(":")[0], "token").write_text("fixture-private-token\n")
    sys.exit(0)
if args[0] == "up":
    sys.exit(7 if args[-1] == "init-packs" and os.environ["FAILURE"] == "up" else 0)
if args[0] == "wait":
    sys.exit(7 if args[-1] == "init-packs" else 0)
if args[0] == "logs":
    print("raw fixture-private-token must not be printed")
    print("ATTUNE_BOOTSTRAP_DIAGNOSTIC stage=publication outcome=failed exit=1")
    sys.exit(0)
if args[0] == "down":
    sys.exit(0)
sys.exit(98)
''')
                docker.chmod(0o755)
                env = dict(os.environ, PATH=f"{root}:" + os.environ["PATH"], EVENTS=str(root / "events"),
                           FAILURE=failure, ATTUNE_E2E_RUN_ID="diagnostic-fixture", ATTUNE_TEST_RUN_ID="diag-fixture")
                for name in ("ATTUNE_BOOTSTRAP_TOKEN", "ATTUNE_E2E_COMPOSE_OVERRIDE", "ATTUNE_E2E_PROJECT_NAME"):
                    env.pop(name, None)
                result = subprocess.run(["bash", str(ROOT / "scripts/run-integration-tests.sh"), "--no-build"],
                                        env=env, capture_output=True, text=True, timeout=15)
                self.assertEqual(result.returncode, 7, result.stdout + result.stderr)
                self.assertIn("stage=publication outcome=failed exit=1", result.stdout)
                self.assertNotIn("fixture-private-token", result.stdout + result.stderr)
                calls = [json.loads(line) for line in (root / "events").read_text().splitlines()]
                logs = next(i for i, call in enumerate(calls) if "logs" in call)
                revoke = next(i for i, call in enumerate(calls) if "revoke" in call)
                down = next(i for i, call in enumerate(calls) if "down" in call)
                self.assertLess(logs, revoke)
                self.assertLess(revoke, down)
                self.assertFalse(any("pytest" in call for call in calls))

    def test_private_host_candidate_reaches_upload_and_http_failure_is_safe(self):
        self.run_initializer(reject=True)

    def test_private_host_candidate_completes_admission(self):
        self.run_initializer(reject=False)

    def run_initializer(self, reject):
        received = []
        active = False
        host = {"x86_64": ("linux-amd64", 62), "aarch64": ("linux-arm64", 183)}[platform.machine()]

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def respond(self, code, body):
                self.send_response(code)
                self.end_headers()
                self.wfile.write(json.dumps(body).encode())

            def do_GET(self):
                if self.path == "/health/ready":
                    self.respond(200, {"pack_publication": {"protocol_version": 1, "client_version": 1,
                        "revision": "authenticated-pack-publication-1", "authorization": "registrar",
                        "components": ["immutable_release", "atomic_deployment"]}})
                elif self.path == "/api/v1/packs/core/releases":
                    self.respond(200, {"data": [{"id": 1, "version": "1.0.7", "digest": "owned", "is_active": True}] if active else []})
                elif self.path == "/health/bootstrap":
                    self.respond(200, {"core": {"release_id": 1, "version": "1.0.7", "digest": "owned"}})
                else:
                    self.respond(404, {})

            def do_POST(self):
                nonlocal active
                body = self.rfile.read(int(self.headers["Content-Length"]))
                if self.path == "/auth/token-login":
                    self.respond(200, {"data": {"access_token": "fixture-access-secret"}})
                    return
                self.assert_upload(body)
                received.append(body)
                if reject:
                    self.respond(422, {"error": "fixture-secret-must-never-be-printed", "details": {
                        "stage": "metadata_validation", "diagnostics": [
                            {"code": "metadata.invalid_field", "message": "fixture-access-secret"}]}})
                else:
                    active = True
                    self.respond(201, {})

            def assert_upload(self, body):
                assert self.path == "/api/v1/packs/upload"
                assert self.headers["Authorization"] == "Bearer fixture-access-secret"

        with tempfile.TemporaryDirectory(prefix="core-bootstrap-failure-") as directory:
            root = Path(directory)
            shutil.copytree(ROOT / "packs/core", root / "source/core")
            (root / "active").mkdir()
            (root / "active/sentinel").write_text("unchanged")
            for action in ("echo", "sleep", "noop", "http_request"):
                binary = root / "staging/core/actions/bin" / action / host[0]
                binary.parent.mkdir(parents=True)
                header = bytearray(64)
                header[:7] = b"\x7fELF\x02\x01\x01"
                header[16:18] = (2).to_bytes(2, "little")
                header[18:20] = host[1].to_bytes(2, "little")
                binary.write_bytes(header)
                binary.chmod(0o755)
            tools = root / "tools"
            tools.mkdir()
            for tool in ("pip", "chown"):
                (tools / tool).write_text("#!/bin/sh\nexit 0\n")
                (tools / tool).chmod(0o755)
            # Remap only the container publisher path to the actual tracked script.
            (tools / "python3").write_text('#!/bin/sh\nshift\nexec "$REAL_PYTHON" "$PUBLISHER" "$@"\n')
            (tools / "python3").chmod(0o755)
            env = dict(os.environ, PATH=f"{tools}:/usr/bin:/bin", REAL_PYTHON=sys.executable,
                PUBLISHER=str(ROOT / "scripts/bootstrap_core_pack.py"),
                ATTUNE_BOOTSTRAP_MODE="volume", ATTUNE_BOOTSTRAP_TOKEN="fixture-integration-secret",
                SOURCE_PACKS_DIR=str(root / "source"), PACK_BINARIES_DIR=str(root / "staging"),
                TARGET_PACKS_DIR=str(root / "active"), RUNTIME_ENVS_DIR=str(root / "runtime"),
                ARTIFACTS_DIR=str(root / "artifacts"), TMPDIR=str(root))
            with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
                thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01})
                thread.start()
                try:
                    env["ATTUNE_API_URL"] = f"http://127.0.0.1:{server.server_port}"
                    result = subprocess.run(["sh", str(ROOT / "docker/init-packs.sh")], env=env,
                        capture_output=True, text=True, timeout=15)
                finally:
                    server.shutdown()
                    thread.join(timeout=5)
                    self.assertFalse(thread.is_alive())
            self.assertEqual(result.returncode, 1 if reject else 0, result.stderr)
            self.assertEqual(len(received), 1)
            # Inspect the real multipart payload, not a mocked publisher invocation.
            body = received[0]
            self.assertIn(b'name="absent_metadata_policy"\r\n\r\nremove', body)
            start = body.index(b"\x1f\x8b")
            with tarfile.open(fileobj=io.BytesIO(body[start:]), mode="r:gz") as archive:
                names = archive.getnames()
                for action in ("echo", "sleep", "noop", "http_request"):
                    self.assertIn(f"core/actions/bin/{action}/{host[0]}", names)
                self.assertNotIn("core/actions/run_agent_command.yaml", names)
                self.assertEqual(archive.extractfile("core/sensors/timer_sensor.yaml").read(),
                                 (ROOT / "packs/core/sensors/timer_sensor.yaml").read_bytes())
            self.assertEqual(list((root / "active").iterdir()), [root / "active/sentinel"])
            self.assertFalse(list(root.glob("tmp.*")), "private candidate must be removed on success/failure")
            self.assertNotIn("fixture-secret", result.stdout + result.stderr)
            self.assertNotIn("fixture-access-secret", result.stdout + result.stderr)
            self.assertNotIn("fixture-integration-secret", result.stdout + result.stderr)
            if reject:
                self.assertIn("http=422 operation=upload api_stage=metadata_validation code=metadata.invalid_field", result.stderr)
                self.assertIn("stage=publication outcome=failed exit=1", result.stderr)


if __name__ == "__main__":
    unittest.main()
