#!/usr/bin/env python3
"""Real owned TLS rejection plus finite HTTP denial/privacy regressions."""
import http.server
import io
import json
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from workload_project_probes import caller_rejection, tls_rejection


class TlsServer(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", "0")
        self.end_headers()


class ProjectProbeTests(unittest.TestCase):
    def test_trusted_positive_and_real_untrusted_ca_wrong_hostname_negatives(self):
        with tempfile.TemporaryDirectory(prefix="attune-owned-tls-probe-") as directory:
            root = Path(directory)
            key, cert = root / "key.pem", root / "certificate.pem"
            subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
                "-keyout", str(key), "-out", str(cert)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            key.chmod(0o600)
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), TlsServer)
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(cert, key)
            server.socket = context.wrap_socket(server.socket, server_side=True)
            worker = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01})
            worker.start()
            try:
                trusted = ssl.create_default_context(cafile=str(cert))
                client = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=trusted))
                with client.open(f"https://127.0.0.1:{server.server_port}/", timeout=3) as response:
                    self.assertEqual(response.status, 200)
                evidence = []
                tls_rejection(server.server_port, ssl.create_default_context(), "127.0.0.1", "untrusted_ca", evidence.append)
                tls_rejection(server.server_port, trusted, "wrong-owned-hostname.invalid", "wrong_hostname", evidence.append)
                self.assertEqual([item["exception_category"] for item in evidence], ["untrusted_certificate", "hostname_verification"])
                self.assertTrue(all(type(item["ssl_verify_code"]) is int for item in evidence))
            finally:
                server.shutdown()
                worker.join(timeout=5)
                server.server_close()
                self.assertFalse(worker.is_alive())

    def test_only_401_and_403_are_denials_and_no_response_body_reason_or_grant_is_recorded(self):
        class Client:
            def __init__(self, code):
                self.code = code
                self.body = io.BytesIO(b"owned-private-response-body")

            def open(self, request, timeout):
                self.seen = request
                raise urllib.error.HTTPError(request.full_url, self.code, "owned-private-reason", {}, self.body)

        for code in [200, 400, 401, 403, 404, 429, 500, 503]:
            evidence = []
            client = Client(code)
            invoke = lambda: caller_rejection(client, "https://127.0.0.1:1", b"owned-private-input",
                {"x-attune-launch-dispatch": "owned-private-grant"}, "wrong_grant", evidence.append)
            if code in {401, 403}:
                invoke()
            else:
                with self.assertRaises(RuntimeError):
                    invoke()
            self.assertEqual(evidence[0]["http_status"], code)
            self.assertEqual(evidence[0]["subphase"], "wrong_grant")
            self.assertTrue(client.body.closed)
            self.assertNotIn("owned-private", json.dumps(evidence))

    def test_unexpected_tls_acceptance_is_explicit_failure_not_a_passing_negative(self):
        # No private key/cert/exception details are serialized by this probe.
        from unittest.mock import MagicMock, patch
        context = MagicMock()
        evidence = []
        with patch("workload_project_probes.socket.create_connection"):
            with self.assertRaises(RuntimeError):
                tls_rejection(8443, context, "localhost", "untrusted_ca", evidence.append)
        self.assertEqual(evidence, [{"phase": "tls-and-caller-denials", "subphase": "untrusted_ca",
                                    "outcome": "unexpected_tls_acceptance"}])

    def test_accepted_http_response_is_reported_and_never_passes_denial(self):
        from unittest.mock import MagicMock
        client = MagicMock()
        client.open.return_value.__enter__.return_value.status = 200
        evidence = []
        with self.assertRaises(RuntimeError):
            caller_rejection(client, "https://127.0.0.1:1", b"private-input", {}, "missing_grant", evidence.append)
        self.assertEqual(evidence, [{"phase": "tls-and-caller-denials", "subphase": "missing_grant",
                                    "outcome": "unexpected_http_acceptance", "http_status": 200}])


if __name__ == "__main__":
    unittest.main()
