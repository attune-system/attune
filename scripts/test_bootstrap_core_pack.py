import importlib.util
import json
import os
import sys
import tempfile
import threading
import unittest
import urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).with_name("bootstrap_core_pack.py")
SPEC = importlib.util.spec_from_file_location("bootstrap_core_pack", SCRIPT)
bootstrap = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bootstrap)

READY = {"pack_publication": {"protocol_version": 1, "client_version": 1, "revision": bootstrap.BOOTSTRAP_REVISION, "authorization": "registrar", "components": sorted(bootstrap.BOOTSTRAP_COMPONENTS)}}


class BootstrapTransportTests(unittest.TestCase):
    def test_non_http_schemes_and_embedded_credentials_are_rejected_before_io(self):
        for url in ("file:///etc/hosts", "ftp://example.invalid/core", "data:text/plain,secret",
                    "http:///missing-host", "https://user:password@example.invalid/core"):
            with self.subTest(url=url), mock.patch.object(bootstrap.urllib.request, "build_opener") as opener:
                with self.assertRaises(ValueError):
                    bootstrap.request(url)
                opener.assert_not_called()

    def test_http_works_but_redirects_never_forward_publication_credentials(self):
        received = []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                received.append((self.path, self.headers.get("Authorization")))
                if self.path == "/ready":
                    self.send_response(200)
                    self.end_headers()
                    self.wfile.write(b'{"ready":true}')
                else:
                    self.send_response(302)
                    self.send_header("Location", self.path.removeprefix("/redirect/"))
                    self.end_headers()

        with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01})
            thread.start()
            base_url = f"http://127.0.0.1:{server.server_port}"
            try:
                with bootstrap.request(base_url + "/ready") as response:
                    self.assertEqual(json.load(response), {"ready": True})
                for destination in (base_url + "/ready", "http://example.invalid/leak", "file:///etc/hosts"):
                    with self.subTest(destination=destination):
                        with self.assertRaises(urllib.error.HTTPError) as error:
                            bootstrap.request(base_url + "/redirect/" + destination,
                                              headers={"Authorization": "Bearer fixture"})
                        error.exception.close()
                self.assertEqual(len(received), 4, "redirect targets must never receive a request")
            finally:
                server.shutdown()
                thread.join(timeout=5)
                self.assertFalse(thread.is_alive())


class BootstrapCorePackTests(unittest.TestCase):
    def test_authenticated_publication_does_not_enable_managed_runtime_exemptions(self):
        ready = {
            "pack_publication": {
                "protocol_version": 1,
                "client_version": 1,
                "revision": "authenticated-pack-publication-1",
                "authorization": "registrar",
                "components": ["immutable_release", "atomic_deployment"],
            },
            "bootstrap_admission_protocol": None,
            "bootstrap_admission": {"enabled": False},
        }
        bootstrap.require_contract(ready)

    def test_publication_requires_the_exact_authenticated_contract(self):
        for field, invalid in [
            ("protocol_version", True),
            ("client_version", 2),
            ("revision", "unknown"),
            ("authorization", "anonymous"),
            ("components", []),
        ]:
            with self.subTest(field=field):
                ready = {"pack_publication": {**READY["pack_publication"], field: invalid}}
                with self.assertRaisesRegex(RuntimeError, "publication contract"):
                    bootstrap.require_contract(ready)
        with self.assertRaisesRegex(RuntimeError, "publication contract"):
            bootstrap.require_contract({"pack_publication": None})

    def test_required_digest_is_sent_to_atomic_upload_not_just_checked_afterwards(self):
        response = mock.MagicMock(status=201)
        response.__enter__.return_value = response
        with mock.patch.object(bootstrap, "request", return_value=response) as request, mock.patch.object(bootstrap, "deterministic_archive", return_value=b"archive"):
            bootstrap.upload("http://api", "access", Path("/owned/core"), "a" * 64)
        body = request.call_args.kwargs["data"]
        self.assertIn(b'name="expected_release_digest"', body)
        self.assertIn(b"a" * 64, body)

    def setUp(self):
        admission = mock.patch.object(bootstrap, "require_active_admission")
        self.admission = admission.start()
        self.addCleanup(admission.stop)

    def test_matching_but_unadmitted_release_is_not_success(self):
        with tempfile.TemporaryDirectory() as directory:
            pack_dir = Path(directory)
            (pack_dir / "pack.yaml").write_text('ref: core\nversion: "1.0.2"\n')
            self.admission.side_effect = RuntimeError("missing runtime admission")
            with mock.patch.object(bootstrap, "get_active_release", return_value={"version":"1.0.2", "digest":"abc"}), \
                 self.assertRaisesRegex(RuntimeError, "missing runtime admission"):
                bootstrap.reconcile("http://api", "token", pack_dir, "1.0.2", None)

    def test_failed_upgrade_propagates_without_deleting_active_release(self):
        with tempfile.TemporaryDirectory() as directory:
            pack_dir = Path(directory)
            (pack_dir / "pack.yaml").write_text('ref: core\nversion: "1.0.2"\n')
            with mock.patch.object(bootstrap, "get_active_release", return_value={"version": "1.0.1", "digest": "old"}) as lookup, \
                 mock.patch.object(bootstrap, "upload", side_effect=RuntimeError("admission denied")), \
                 self.assertRaisesRegex(RuntimeError, "admission denied"):
                bootstrap.reconcile("http://api", "token", pack_dir, "1.0.2", None)
            self.assertEqual(lookup.call_count, 1)

    def test_common_publisher_uses_existing_credential_without_db_mutation(self):
        with mock.patch.dict(os.environ, {"ATTUNE_BOOTSTRAP_TOKEN": "existing"}, clear=True), \
             mock.patch.object(bootstrap, "wait_for_api"), \
             mock.patch.object(bootstrap, "bundled_pack_version", return_value="1.0.2"), \
             mock.patch.object(bootstrap, "token_login", return_value="access") as login, \
             mock.patch.object(bootstrap, "reconcile", side_effect=RuntimeError("admission denied")), \
             self.assertRaisesRegex(RuntimeError, "admission denied"):
             bootstrap.publish_core("http://api", Path("/source/packs/core"))
        login.assert_called_once_with("http://api", "existing")
        source = SCRIPT.read_text()
        self.assertNotIn("psycopg2", source)
        self.assertNotIn("INSERT INTO identity", source)

    def test_publish_does_not_wait_for_workers_blocked_on_bootstrap_job(self):
        with mock.patch.dict(os.environ, {"ATTUNE_API_URL": "http://api"}), \
             mock.patch.object(sys, "argv", [str(SCRIPT), "publish"]), \
             mock.patch.object(bootstrap, "wait_for_api"), \
             mock.patch.object(bootstrap, "publish_core") as publish, \
             mock.patch.object(bootstrap, "wait_for_core") as wait:
            bootstrap.main()
        publish.assert_called_once()
        wait.assert_not_called()

    def test_matching_active_release_skips_upload(self):
        with tempfile.TemporaryDirectory() as directory:
            pack_dir = Path(directory)
            (pack_dir / "pack.yaml").write_text(
                'ref: core\nversion: "1.0.2"\n', encoding="utf-8"
            )

            with (
                mock.patch.object(
                    bootstrap,
                    "get_active_release",
                    return_value={
                        "version": "1.0.2",
                        "digest": "abc",
                        "is_active": True,
                    },
                ),
                mock.patch.object(bootstrap, "upload") as upload,
            ):
                changed = bootstrap.reconcile(
                    "http://api", "token", pack_dir, "1.0.2", None
                )

        self.assertFalse(changed)
        upload.assert_not_called()

    def test_different_active_release_uploads(self):
        with tempfile.TemporaryDirectory() as directory:
            pack_dir = Path(directory)
            (pack_dir / "pack.yaml").write_text(
                'ref: core\nversion: "1.0.2"\n', encoding="utf-8"
            )

            with (
                mock.patch.object(
                    bootstrap,
                    "get_active_release",
                    side_effect=[
                        {"version": "1.0.1", "digest": "old", "is_active": True},
                        {"version": "1.0.2", "digest": "new", "is_active": True},
                    ],
                ),
                mock.patch.object(bootstrap, "upload") as upload,
            ):
                changed = bootstrap.reconcile(
                    "http://api", "token", pack_dir, "1.0.2", "new"
                )

        self.assertTrue(changed)
        upload.assert_called_once_with("http://api", "token", pack_dir, "new")

    def test_matching_version_with_wrong_digest_fails_without_upload(self):
        with tempfile.TemporaryDirectory() as directory:
            pack_dir = Path(directory)
            (pack_dir / "pack.yaml").write_text(
                'ref: core\nversion: "1.0.2"\n', encoding="utf-8"
            )

            with (
                mock.patch.object(
                    bootstrap,
                    "get_active_release",
                    return_value={
                        "version": "1.0.2",
                        "digest": "unexpected",
                        "is_active": True,
                    },
                ),
                mock.patch.object(bootstrap, "upload") as upload,
                self.assertRaisesRegex(RuntimeError, "expected wanted"),
            ):
                bootstrap.reconcile(
                    "http://api", "token", pack_dir, "1.0.2", "wanted"
                )

        upload.assert_not_called()

    def test_chart_and_bundled_pack_versions_must_match(self):
        with tempfile.TemporaryDirectory() as directory:
            pack_dir = Path(directory)
            (pack_dir / "pack.yaml").write_text(
                'ref: core\nversion: "1.0.1"\n', encoding="utf-8"
            )

            with self.assertRaisesRegex(RuntimeError, "expected core pack version"):
                bootstrap.reconcile(
                    "http://api", "token", pack_dir, "1.0.2", None
                )

    def test_pack_release_lookup_returns_active_release(self):
        response = mock.MagicMock(status=200)
        response.__enter__.return_value = response
        response.read.return_value = json.dumps(
            {
                "data": [
                    {"version": "1.0.1", "digest": "old", "is_active": False},
                    {"version": "1.0.2", "digest": "new", "is_active": True},
                ]
            }
        ).encode()
        response.__iter__.return_value = iter(response.read.return_value.splitlines())
        with mock.patch.object(bootstrap, "request", return_value=response):
            active = bootstrap.get_active_release("http://api", "token")

        self.assertEqual(active["version"], "1.0.2")

    def test_api_wait_uses_platform_readiness(self):
        response = mock.MagicMock(status=200)
        response.__enter__.return_value = response
        response.read.return_value = json.dumps(READY).encode()
        with mock.patch.object(bootstrap, "request", return_value=response) as request:
            bootstrap.wait_for_api("http://api", bootstrap.time.monotonic() + 10)

        request.assert_called_once_with("http://api/health/ready")

    def test_old_api_is_rejected_before_bootstrap_can_publish(self):
        response = mock.MagicMock(status=200)
        response.__enter__.return_value = response
        response.read.return_value = b'{"status":"ready"}'
        with mock.patch.object(bootstrap, "request", return_value=response), \
             self.assertRaisesRegex(RuntimeError, "compatible API/bootstrap versions"):
            bootstrap.wait_for_api("http://api", bootstrap.time.monotonic() + 10)

    def test_content_wait_uses_transitional_content_health(self):
        response = mock.MagicMock(status=200)
        response.__enter__.return_value = response
        with mock.patch.object(bootstrap, "request", return_value=response) as request:
            bootstrap.wait_for_core("http://api", bootstrap.time.monotonic() + 10)

        request.assert_called_once_with("http://api/health/content")

    def test_timeout_is_capped_at_300_seconds(self):
        with (
            mock.patch.dict(
                os.environ,
                {
                    "ATTUNE_API_URL": "http://api",
                    "ATTUNE_BOOTSTRAP_TIMEOUT_SECONDS": "999",
                },
                clear=False,
            ),
            mock.patch.object(sys, "argv", [str(SCRIPT), "wait"]),
            mock.patch.object(bootstrap.time, "monotonic", return_value=100),
            mock.patch.object(bootstrap, "wait_for_api") as wait_for_api,
            mock.patch.object(bootstrap, "wait_for_core") as wait_for_core,
        ):
            bootstrap.main()

        wait_for_api.assert_called_once_with("http://api", 400)
        wait_for_core.assert_called_once_with("http://api", 400)


if __name__ == "__main__":
    unittest.main()
