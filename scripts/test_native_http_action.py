#!/usr/bin/env python3
"""Real native process against owned loopback HTTP/TLS mocks only; no worker tools."""
import base64
import json
import os
from pathlib import Path
import ssl
import socket
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

    def server(self, *, tls=False, dual_tls=False, redirect=None, delay=0, body_delay=0, payload=None):
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
                destination = redirect(record) if callable(redirect) else redirect
                if destination:
                    status, target = destination
                    self.send_response(status)
                    self.send_header("Location", target)
                    self.end_headers()
                    return
                data = payload if payload is not None else json.dumps(record, ensure_ascii=False).encode()
                self.send_response(418 if self.path == "/failure" else 200)
                self.send_header("Content-Type", "application/json")
                self.send_header("X-Quote", 'a "quote"')
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                try:
                    if body_delay:
                        self.wfile.flush()
                        time.sleep(body_delay)
                    if self.command != "HEAD":
                        self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass  # The timeout test intentionally closes its own client.

            do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = do_OPTIONS = do_GET

        context = None

        class Server(ThreadingHTTPServer):
            def get_request(self):
                stream, address = super().get_request()
                if dual_tls:
                    stream.settimeout(5)
                    if stream.recv(1, socket.MSG_PEEK) == b"\x16":
                        stream = context.wrap_socket(stream, server_side=True)
                return stream, address

        server = Server(("127.0.0.1", 0), Handler)
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
            if not dual_tls:
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

    def test_redirect_origin_credentials_and_later_hop_validation(self):
        credentials = {"Authorization": "Bearer private", "Cookie": "session=private",
                       "Proxy-Authorization": "Basic private"}
        # Relative redirects retain all three headers on the same origin.
        same = self.server(redirect=lambda r: (307, "/final") if r["path"] == "/start" else None)
        output = self.invoke({"url": same + "/start", "headers": credentials})
        for name, value in credentials.items():
            self.assertEqual(output["json"]["headers"][name.lower()], value)
        # Same port/scheme, hostname only changes (both addresses are loopback).
        host = self.server(redirect=lambda r: (307, "http://localhost:" + host.rsplit(":", 1)[1] + "/final") if r["path"] == "/start" else None)
        output = self.invoke({"url": host + "/start", "headers": credentials})
        for name in credentials:
            self.assertNotIn(name.lower(), output["json"]["headers"])
        # Same host AND port, scheme alone changes. The owned server accepts TLS
        # and plaintext on its one ephemeral listener, so no port change masks it.
        scheme = self.server(tls=True, dual_tls=True, redirect=lambda r: (307, scheme.replace("https:", "http:") + "/final") if r["path"] == "/start" else None)
        output = self.invoke({"url": scheme + "/start", "verify_ssl": False, "headers": credentials})
        for name in credentials:
            self.assertNotIn(name.lower(), output["json"]["headers"])
        for unsafe in ("file:///etc/passwd", "http://user:private@127.0.0.1/"):
            later = self.server(redirect=(307, unsafe))
            first = self.server(redirect=(307, later))
            output = self.invoke({"url": first}, success=False)
            self.assertEqual(output["status_code"], 0)
            self.assertNotIn("private", output["error"])

    def test_response_size_types_and_complete_request_deadline(self):
        for data, expected in ((b'[1,"two"]', [1, "two"]), (b'42', None), (b'plain', None)):
            output = self.invoke({"url": self.server(payload=data)})
            self.assertEqual(output["json"], expected)
            self.assertEqual(output["body"], data.decode())
        limit = 16 * 1024 * 1024
        self.assertEqual(len(self.invoke({"url": self.server(payload=b"x" * limit)})["body"]), limit)
        output = self.invoke({"url": self.server(payload=b"x" * (limit + 1))}, success=False)
        self.assertIn("16 MiB", output["error"])
        slow = self.server(body_delay=2)
        started = time.monotonic()
        self.invoke({"url": slow, "timeout": 1}, success=False)
        self.assertLess(time.monotonic() - started, 2)
        later = self.server(delay=0.7)
        first = self.server(delay=0.7, redirect=(307, later))
        started = time.monotonic()
        self.invoke({"url": first, "timeout": 1}, success=False)
        self.assertLess(time.monotonic() - started, 2)

    def test_malformed_and_physical_line_stdin_rejected(self):
        base = self.server()
        for raw in (b'url="' + base.encode() + b'"\nmethod=POST\n', b'{"url":', b'[]', b'{"url":"' + base.encode() + b'"}\n{"method":"POST"}'):
            result = subprocess.run([str(self.binary)], input=raw,
                                    env={"PATH": "/no-tools-or-interpreters"}, capture_output=True, timeout=8)
            self.assertEqual(result.returncode, 1)
            self.assertEqual(result.stderr, b"")
            self.assertFalse(json.loads(result.stdout)["success"])
        self.assertEqual(self.requests, [])

    def test_timeout_is_bounded(self):
        base = self.server(delay=2)
        output = self.invoke({"url": base, "timeout": 1}, success=False)
        self.assertEqual(output["status_code"], 0)
        self.assertIn("timed out", output["error"])


if __name__ == "__main__":
    unittest.main()
