#!/usr/bin/env python3
"""Real native process against owned loopback HTTP/TLS mocks only; no worker tools."""
import base64
import json
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class NativeHttpTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.binary = Path(os.environ["ATTUNE_CORE_HTTP_BINARY"]).resolve(strict=True)

    def setUp(self):
        self.requests = []
        self.servers = []
        self.owned = tempfile.TemporaryDirectory(prefix="native-http-test-")
        self.addCleanup(self.owned.cleanup)
        self.addCleanup(self.stop_servers)

    def stop_servers(self):
        for server, thread in self.servers:
            server.shutdown()
            thread.join(timeout=5)
            server.server_close()
            self.assertFalse(thread.is_alive())

    def server(self, *, tls=False, redirect=None, delay=0):
        received = self.requests

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
                record = {"method": self.command, "path": self.path,
                          "headers": {name.lower(): value for name, value in self.headers.items()}, "body": body.decode()}
                received.append(record)
                if delay:
                    time.sleep(delay)
                if redirect:
                    status, target = redirect
                    self.send_response(status)
                    self.send_header("Location", target)
                    self.end_headers()
                    return
                data = json.dumps(record, ensure_ascii=False).encode()
                self.send_response(418 if self.path == "/failure" else 200)
                self.send_header("Content-Type", "application/json")
                self.send_header("X-Quote", 'a "quote"')
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                try:
                    if self.command != "HEAD":
                        self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass  # The timeout test intentionally closes its own client.

            do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = do_OPTIONS = do_GET

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.daemon_threads = False
        if tls:
            root = Path(self.owned.name)
            subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                            "-keyout", str(root / "key.pem"), "-out", str(root / "cert.pem"),
                            "-days", "1", "-subj", "/CN=localhost",
                            "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True, timeout=15)
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(root / "cert.pem", root / "key.pem")
            server.socket = context.wrap_socket(server.socket, server_side=True)
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01})
        thread.start()
        self.servers.append((server, thread))
        return f"{'https' if tls else 'http'}://127.0.0.1:{server.server_port}"

    def invoke(self, parameters, *, success=True):
        result = subprocess.run([str(self.binary)], input=json.dumps(parameters).encode(),
                                env={"PATH": "/no-tools-or-interpreters"}, capture_output=True, timeout=8)
        self.assertEqual(result.returncode, 0 if success else 1, result.stdout.decode())
        self.assertEqual(result.stderr, b"")
        output = json.loads(result.stdout)
        self.assertEqual(output["success"], success)
        return output

    def test_binary_body_quotes_unicode_newlines_and_query_round_trip(self):
        base = self.server()
        body = "雪 '\" \\ \nurl=--help\n\0\n\n"
        for method in ("GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"):
            output = self.invoke({"url": base + "/request?existing=1", "method": method,
                                  "headers": {"X-Text": 'quotes "and" spaces'}, "body": body,
                                  "query_params": {"a&b": "雪\n&=+'", "bool": True}})
            self.assertEqual(output["json"]["body"], body)
            self.assertEqual(output["json"]["method"], method)
            self.assertEqual(output["json"]["headers"]["x-text"], 'quotes "and" spaces')
            from urllib.parse import parse_qs, urlsplit
            query = parse_qs(urlsplit(output["json"]["path"]).query)
            self.assertEqual(query["a&b"], ["雪\n&=+'"])
            self.assertEqual(output["headers"]["x-quote"], 'a "quote"')
        output = self.invoke({"url": base, "method": "POST", "body": "ignored", "json_body": {"雪": "'\"\n"}})
        self.assertEqual(json.loads(output["json"]["body"]), {"雪": "'\"\n"})
        self.assertEqual(output["json"]["headers"]["content-type"], "application/json")
        self.assertEqual(self.invoke({"url": base, "method": "HEAD"})["body"], "")

    def test_options_protocols_and_header_auth_injection_fail_before_io(self):
        base = self.server()
        for url in ("--help", "-o", "file:///etc/passwd", "ftp://127.0.0.1/x", "http://user:secret@127.0.0.1", base + "/\nurl=--help"):
            self.invoke({"url": url}, success=False)
        for values in ({"headers": {"X-Text": "hello\r\nInjected: yes"}},
                       {"headers": {"Bad\nName": "value"}},
                       {"auth_type": "bearer", "auth_token": "secret\r\nX: bad"},
                       {"auth_type": "basic", "auth_username": "name", "auth_password": "secret\nX: bad"},
                       {"timeout": 0}, {"method": "GET\r\nX: bad"}):
            output = self.invoke(dict(url=base, **values), success=False)
            self.assertNotIn("secret", output["error"])
        self.assertEqual(self.requests, [])

    def test_http_error_preserves_response_and_is_not_success(self):
        output = self.invoke({"url": self.server() + "/failure"}, success=False)
        self.assertEqual(output["status_code"], 418)
        self.assertEqual(output["json"]["path"], "/failure")

    def test_authentication_and_cross_origin_redirects(self):
        target = self.server()
        first = self.server(redirect=(307, target + "/final"))
        output = self.invoke({"url": first, "method": "POST", "body": "preserved\n雪",
                              "auth_type": "bearer", "auth_token": "test-secret",
                              "headers": {"Cookie": "session=private"}})
        self.assertEqual(self.requests[0]["headers"]["authorization"], "Bearer test-secret")
        self.assertNotIn("authorization", output["json"]["headers"])
        self.assertNotIn("cookie", output["json"]["headers"])
        self.assertEqual(output["json"]["body"], "preserved\n雪")
        basic = self.invoke({"url": target, "auth_type": "basic", "auth_username": "name", "auth_password": "p:a'ss"})
        self.assertEqual(basic["json"]["headers"]["authorization"], "Basic " + base64.b64encode(b"name:p:a'ss").decode())

    def test_redirect_method_limits_and_protocols(self):
        target = self.server()
        first = self.server(redirect=(303, target))
        output = self.invoke({"url": first, "method": "POST", "body": "removed"})
        self.assertEqual(output["json"]["method"], "GET")
        self.assertEqual(output["json"]["body"], "")
        stopped = self.invoke({"url": first, "follow_redirects": False}, success=False)
        self.assertEqual(stopped["status_code"], 303)
        self.invoke({"url": first, "max_redirects": 0}, success=False)
        evil = self.server(redirect=(302, "file:///etc/passwd"))
        self.invoke({"url": evil}, success=False)
        self.assertFalse(any(r["path"].startswith("file:") for r in self.requests))

    def test_tls_verification_defaults_secure_and_redirect_cannot_switch_protocol(self):
        target = self.server(tls=True)
        self.invoke({"url": target}, success=False)
        self.assertEqual(self.requests, [])
        output = self.invoke({"url": target, "verify_ssl": False})
        self.assertEqual(output["status_code"], 200)
        evil = self.server(tls=True, redirect=(307, "ftp://127.0.0.1/private"))
        self.invoke({"url": evil, "verify_ssl": False}, success=False)
        plain = self.server()
        downgrade = self.server(tls=True, redirect=(307, plain))
        output = self.invoke({"url": downgrade, "verify_ssl": False, "auth_type": "bearer", "auth_token": "private"})
        self.assertNotIn("authorization", output["json"]["headers"])

    def test_timeout_is_bounded(self):
        base = self.server(delay=2)
        output = self.invoke({"url": base, "timeout": 1}, success=False)
        self.assertEqual(output["status_code"], 0)
        self.assertIn("timed out", output["error"])


if __name__ == "__main__":
    unittest.main()
