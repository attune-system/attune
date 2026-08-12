#!/usr/bin/env python3
"""Behavior and schema tests for core pack actions."""

import json
import os
import subprocess
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit


def flatten_dotenv(params, prefix=""):
    """Format params like the worker's stdin/dotenv parameter delivery."""
    lines = []
    for key, value in params.items():
        name = f"{prefix}.{key}" if prefix else key
        if isinstance(value, dict):
            lines.extend(flatten_dotenv(value, name))
            continue
        if isinstance(value, list):
            value = json.dumps(value, separators=(",", ":"))
        elif value is True:
            value = "true"
        elif value is False:
            value = "false"
        elif value is None:
            value = ""
        else:
            value = str(value)
        lines.append(f"{name}='{value.replace(chr(39), chr(39) + chr(92) + chr(39) + chr(39))}'")
    return sorted(lines)


class CorePackTestCase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.pack_dir = Path(__file__).resolve().parent.parent
        cls.actions_dir = cls.pack_dir / "actions"

    def run_action(self, script_name, params=None, parameter_format="dotenv", timeout=10):
        script_path = self.actions_dir / script_name
        self.assertTrue(script_path.exists(), f"action entry point is missing: {script_path}")

        if parameter_format == "dotenv":
            stdin = "\n".join(flatten_dotenv(params or {})) + "\n"
        elif parameter_format == "json":
            stdin = json.dumps(params or {}, separators=(",", ":")) + "\n"
        else:
            raise ValueError(f"unsupported test parameter format: {parameter_format}")

        env = os.environ.copy()
        for key in tuple(env):
            if key.startswith("ATTUNE_ACTION_"):
                del env[key]
        command = ["python3", str(script_path)] if script_path.suffix == ".py" else ["/bin/sh", str(script_path)]
        result = subprocess.run(
            command,
            input=stdin,
            text=True,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            cwd=self.actions_dir,
        )
        return result.stdout, result.stderr, result.returncode


class TestEchoAction(CorePackTestCase):
    def test_basic_echo(self):
        stdout, stderr, code = self.run_action("echo.sh", {"message": "Hello, Attune!"})
        self.assertEqual((code, stdout, stderr), (0, "Hello, Attune!", ""))

    def test_omitted_message_is_empty(self):
        stdout, stderr, code = self.run_action("echo.sh")
        self.assertEqual((code, stdout, stderr), (0, "", ""))

    def test_empty_message(self):
        stdout, stderr, code = self.run_action("echo.sh", {"message": ""})
        self.assertEqual((code, stdout, stderr), (0, "", ""))

    def test_special_characters(self):
        message = 'Test!@#$%^&*()[]{}|\\:;"<>,.?/~`'
        stdout, stderr, code = self.run_action("echo.sh", {"message": message})
        self.assertEqual((code, stdout, stderr), (0, message, ""))

    def test_obsolete_parameter_environment_is_ignored(self):
        env = os.environ.copy()
        env["ATTUNE_ACTION_MESSAGE"] = "stale"
        result = subprocess.run(
            ["/bin/sh", str(self.actions_dir / "echo.sh")],
            input="message='stdin'\n",
            text=True,
            env=env,
            capture_output=True,
            check=False,
        )
        self.assertEqual((result.returncode, result.stdout), (0, "stdin"))


class TestNoopAction(CorePackTestCase):
    def test_basic_noop(self):
        stdout, stderr, code = self.run_action("noop.sh")
        self.assertEqual(code, 0)
        self.assertEqual(stderr, "")
        self.assertEqual(stdout, "No operation completed successfully\n")

    def test_message(self):
        stdout, _, code = self.run_action("noop.sh", {"message": "Test message"})
        self.assertEqual(code, 0)
        self.assertEqual(stdout, "[NOOP] Test message\nNo operation completed successfully\n")

    def test_exit_code_boundaries(self):
        for exit_code in (0, 5, 255):
            with self.subTest(exit_code=exit_code):
                _, _, code = self.run_action("noop.sh", {"exit_code": exit_code})
                self.assertEqual(code, exit_code)

    def test_invalid_exit_codes(self):
        for exit_code in (-1, 256, "abc"):
            with self.subTest(exit_code=exit_code):
                _, stderr, code = self.run_action("noop.sh", {"exit_code": exit_code})
                self.assertNotEqual(code, 0)
                self.assertIn("ERROR:", stderr)


class TestSleepAction(CorePackTestCase):
    def test_zero_seconds(self):
        start = time.monotonic()
        stdout, stderr, code = self.run_action("sleep.sh", {"seconds": 0})
        self.assertEqual((code, stdout, stderr), (0, "Slept for 0 seconds\n", ""))
        self.assertLess(time.monotonic() - start, 0.5)

    def test_message_and_duration(self):
        start = time.monotonic()
        stdout, stderr, code = self.run_action(
            "sleep.sh", {"seconds": 1, "message": "Sleeping now..."}
        )
        elapsed = time.monotonic() - start
        self.assertEqual(code, 0)
        self.assertEqual(stderr, "")
        self.assertEqual(stdout, "Sleeping now...\nSlept for 1 seconds\n")
        self.assertGreaterEqual(elapsed, 1.0)
        self.assertLess(elapsed, 1.75)

    def test_default_duration(self):
        start = time.monotonic()
        stdout, _, code = self.run_action("sleep.sh")
        self.assertEqual(code, 0)
        self.assertEqual(stdout, "Slept for 1 seconds\n")
        self.assertGreaterEqual(time.monotonic() - start, 1.0)

    def test_invalid_seconds(self):
        for seconds in (-1, 3601, "abc"):
            with self.subTest(seconds=seconds):
                _, stderr, code = self.run_action("sleep.sh", {"seconds": seconds})
                self.assertNotEqual(code, 0)
                self.assertIn("ERROR:", stderr)


class HttpFixtureHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def log_message(self, _format, *_args):
        pass

    def send_json(self, status=200, payload=None, include_body=True):
        body = json.dumps(payload or {}, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if include_body:
            try:
                self.wfile.write(body)
            except BrokenPipeError:
                pass

    def response_payload(self):
        parsed = urlsplit(self.path)
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length).decode() if length else ""
        return {
            "method": self.command,
            "query": parse_qs(parsed.query),
            "request_body": body,
            "custom_header": self.headers.get("X-Custom-Header"),
        }

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/slow":
            time.sleep(2)
        self.send_json(404 if path == "/missing" else 200, self.response_payload())

    def do_POST(self):
        self.send_json(payload=self.response_payload())

    do_PUT = do_POST
    do_PATCH = do_POST
    do_DELETE = do_POST
    do_OPTIONS = do_POST

    def do_HEAD(self):
        self.send_json(payload=self.response_payload(), include_body=False)


class TestHttpRequestAction(CorePackTestCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), HttpFixtureHandler)
        cls.server_thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.server_thread.start()
        cls.base_url = f"http://127.0.0.1:{cls.server.server_port}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.server_thread.join(timeout=2)

    def request(self, params, timeout=10):
        stdout, stderr, code = self.run_action("http_request.sh", params, timeout=timeout)
        self.assertTrue(stdout, f"action produced no JSON; stderr={stderr!r}")
        return json.loads(stdout), stderr, code

    def test_get_with_query_parameters(self):
        result, stderr, code = self.request({
            "url": f"{self.base_url}/inspect",
            "query_params": {"foo": "bar", "page": "1"},
        })
        self.assertEqual((code, stderr), (0, ""))
        self.assertEqual(result["status_code"], 200)
        self.assertTrue(result["success"])
        self.assertEqual(result["json"]["query"], {"foo": ["bar"], "page": ["1"]})
        self.assertGreaterEqual(result["elapsed_ms"], 0)

    def test_post_body(self):
        payload = {"test": "value", "number": 123}
        result, _, code = self.request({
            "url": f"{self.base_url}/post",
            "method": "POST",
            "body": json.dumps(payload, separators=(",", ":")),
        })
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(result["json"]["request_body"]), payload)

    def test_supported_methods(self):
        for method in ("PUT", "PATCH", "DELETE", "OPTIONS"):
            with self.subTest(method=method):
                result, _, code = self.request({"url": f"{self.base_url}/method", "method": method})
                self.assertEqual(code, 0)
                self.assertEqual(result["status_code"], 200)
                self.assertTrue(result["success"])

    def test_missing_url_fails_with_structured_output(self):
        result, stderr, code = self.request({})
        self.assertNotEqual(code, 0)
        self.assertEqual(stderr, "")
        self.assertFalse(result["success"])
        self.assertEqual(result["error"], "url parameter is required")

    def test_non_2xx_is_reported_without_transport_failure(self):
        result, stderr, code = self.request({"url": f"{self.base_url}/missing"})
        self.assertEqual((code, stderr), (0, ""))
        self.assertEqual(result["status_code"], 404)
        self.assertFalse(result["success"])

    def test_timeout_is_structured_transport_failure(self):
        result, stderr, code = self.request(
            {"url": f"{self.base_url}/slow", "timeout": 1}, timeout=5
        )
        self.assertNotEqual(code, 0)
        self.assertEqual(stderr, "")
        self.assertFalse(result["success"])
        self.assertEqual(result["status_code"], 0)
        self.assertEqual(result["url"], f"{self.base_url}/slow")
        self.assertEqual(result["error"], "curl error code 123")


class TestYAMLSchemas(CorePackTestCase):
    STRUCTURED_ACTIONS = {
        "build_pack_envs.yaml",
        "download_packs.yaml",
        "generate_ssh_key_pair.yaml",
        "get_pack_dependencies.yaml",
        "http_request.yaml",
        "register_packs.yaml",
    }

    def test_pack_yaml_valid(self):
        import yaml

        data = yaml.safe_load((self.pack_dir / "pack.yaml").read_text())
        self.assertEqual(data["ref"], "core")

    def test_action_contracts(self):
        import yaml

        for yaml_file in self.actions_dir.glob("*.yaml"):
            with self.subTest(file=yaml_file.name):
                data = yaml.safe_load(yaml_file.read_text())
                self.assertIn("label", data)
                self.assertIn("ref", data)
                self.assertIn("runner_type", data)
                self.assertEqual(data.get("parameter_delivery"), "stdin")
                self.assertIn(data.get("parameter_format"), {"json", "dotenv"})
                self.assertNotIn("output_schema", data)
                entry_point = data.get("entry_point")
                if entry_point and data["runner_type"] != "native":
                    self.assertTrue((self.actions_dir / entry_point).is_file())
                if yaml_file.name in self.STRUCTURED_ACTIONS:
                    self.assertEqual(data.get("output_format"), "json")
                    self.assertIsInstance(data.get("output"), dict)
                    self.assertTrue(data["output"])
                    for field_schema in data["output"].values():
                        self.assertIsInstance(field_schema.get("type"), str)

    def test_script_entry_points_are_executable(self):
        import yaml

        for yaml_file in self.actions_dir.glob("*.yaml"):
            data = yaml.safe_load(yaml_file.read_text())
            entry_point = data.get("entry_point")
            path = self.actions_dir / entry_point if entry_point else None
            if path and path.suffix in {".sh", ".py"}:
                with self.subTest(entry_point=entry_point):
                    self.assertTrue(os.access(path, os.X_OK))


if __name__ == "__main__":
    unittest.main(verbosity=2)
