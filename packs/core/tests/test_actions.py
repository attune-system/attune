#!/usr/bin/env python3
"""
Unit tests for Core Pack Actions

This test suite validates all core pack actions to ensure they behave correctly
with various inputs, handle errors appropriately, and produce expected outputs.

Usage:
    python3 test_actions.py
    python3 -m pytest test_actions.py -v
"""

import json
import os
import subprocess
import sys
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse


class CorePackTestCase(unittest.TestCase):
    """Base test case for core pack tests"""

    @classmethod
    def setUpClass(cls):
        """Set up test environment"""
        # Get the actions directory
        cls.test_dir = Path(__file__).parent
        cls.pack_dir = cls.test_dir.parent
        cls.actions_dir = cls.pack_dir / "actions"

        # Verify actions directory exists
        if not cls.actions_dir.exists():
            raise RuntimeError(f"Actions directory not found: {cls.actions_dir}")

        # Check for required executables
        cls.has_python = cls._check_command("python3")
        cls.has_bash = cls._check_command("bash")

    @staticmethod
    def _check_command(command):
        """Check if a command is available"""
        try:
            subprocess.run(
                [command, "--version"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=2,
            )
            return True
        except (subprocess.TimeoutExpired, FileNotFoundError):
            return False

    def run_action(self, script_name, env_vars=None, expect_failure=False):
        """
        Run an action script with environment variables

        Args:
            script_name: Name of the script file
            env_vars: Dictionary of environment variables
            expect_failure: If True, expects the script to fail

        Returns:
            tuple: (stdout, stderr, exit_code)
        """
        script_path = self.actions_dir / script_name
        stdin_data = None
        # The HTTP action is now a POSIX-shell action that receives canonical
        # DOTENV parameters on stdin. Keep the historical test call shape while
        # exercising the current action contract.
        if script_name == "http_request.py" and not script_path.exists():
            script_name = "http_request.sh"
            script_path = self.actions_dir / script_name
            parameters = {}
            for env_name, value in (env_vars or {}).items():
                if not env_name.startswith("ATTUNE_ACTION_"):
                    continue
                key = env_name.removeprefix("ATTUNE_ACTION_").lower()
                if key in {"headers", "query_params"}:
                    for nested_key, nested_value in json.loads(value).items():
                        parameters[f"{key}.{nested_key}"] = nested_value
                else:
                    parameters[key] = value
            stdin_data = "".join(f"{key}={value}\n" for key, value in parameters.items()).encode()

        if not script_path.exists():
            raise FileNotFoundError(f"Script not found: {script_path}")

        # Prepare environment
        env = os.environ.copy()
        if env_vars:
            env.update(env_vars)

        # Determine the command
        if script_name.endswith(".py"):
            cmd = ["python3", str(script_path)]
        elif script_name.endswith(".sh"):
            cmd = ["bash", str(script_path)]
        else:
            raise ValueError(f"Unknown script type: {script_name}")

        # Run the script
        try:
            result = subprocess.run(
                cmd,
                env=env,
                input=stdin_data,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=15,
                cwd=str(self.actions_dir),
            )
            return (
                result.stdout.decode("utf-8"),
                result.stderr.decode("utf-8"),
                result.returncode,
            )
        except subprocess.TimeoutExpired:
            if expect_failure:
                return "", "Timeout", -1
            raise


class TestEchoAction(CorePackTestCase):
    """Tests for core.echo action"""

    def test_basic_echo(self):
        """Test basic echo functionality"""
        stdout, stderr, code = self.run_action(
            "echo.sh", {"ATTUNE_ACTION_MESSAGE": "Hello, Attune!"}
        )
        self.assertEqual(code, 0)
        self.assertIn("Hello, Attune!", stdout)

    def test_default_message(self):
        """Test default message when none provided"""
        stdout, stderr, code = self.run_action("echo.sh", {})
        self.assertEqual(code, 0)
        self.assertIn("Hello, World!", stdout)

    def test_uppercase_conversion(self):
        """Test uppercase conversion"""
        stdout, stderr, code = self.run_action(
            "echo.sh",
            {
                "ATTUNE_ACTION_MESSAGE": "test message",
                "ATTUNE_ACTION_UPPERCASE": "true",
            },
        )
        self.assertEqual(code, 0)
        self.assertIn("TEST MESSAGE", stdout)
        self.assertNotIn("test message", stdout)

    def test_uppercase_false(self):
        """Test uppercase=false preserves case"""
        stdout, stderr, code = self.run_action(
            "echo.sh",
            {
                "ATTUNE_ACTION_MESSAGE": "Mixed Case",
                "ATTUNE_ACTION_UPPERCASE": "false",
            },
        )
        self.assertEqual(code, 0)
        self.assertIn("Mixed Case", stdout)

    def test_empty_message(self):
        """Test empty message"""
        stdout, stderr, code = self.run_action("echo.sh", {"ATTUNE_ACTION_MESSAGE": ""})
        self.assertEqual(code, 0)
        # Empty message should produce a newline
        # bash echo with empty string still outputs newline

    def test_special_characters(self):
        """Test message with special characters"""
        special_msg = "Test!@#$%^&*()[]{}|\\:;\"'<>,.?/~`"
        stdout, stderr, code = self.run_action(
            "echo.sh", {"ATTUNE_ACTION_MESSAGE": special_msg}
        )
        self.assertEqual(code, 0)
        self.assertIn(special_msg, stdout)

    def test_multiline_message(self):
        """Test message with newlines"""
        multiline_msg = "Line 1\nLine 2\nLine 3"
        stdout, stderr, code = self.run_action(
            "echo.sh", {"ATTUNE_ACTION_MESSAGE": multiline_msg}
        )
        self.assertEqual(code, 0)
        # Depending on shell behavior, newlines might be interpreted


class TestNoopAction(CorePackTestCase):
    """Tests for core.noop action"""

    def test_basic_noop(self):
        """Test basic noop functionality"""
        stdout, stderr, code = self.run_action("noop.sh", {})
        self.assertEqual(code, 0)
        self.assertIn("No operation completed successfully", stdout)

    def test_noop_with_message(self):
        """Test noop with custom message"""
        stdout, stderr, code = self.run_action(
            "noop.sh", {"ATTUNE_ACTION_MESSAGE": "Test message"}
        )
        self.assertEqual(code, 0)
        self.assertIn("Test message", stdout)
        self.assertIn("No operation completed successfully", stdout)

    def test_custom_exit_code_success(self):
        """Test custom exit code 0"""
        stdout, stderr, code = self.run_action(
            "noop.sh", {"ATTUNE_ACTION_EXIT_CODE": "0"}
        )
        self.assertEqual(code, 0)

    def test_custom_exit_code_failure(self):
        """Test custom exit code non-zero"""
        stdout, stderr, code = self.run_action(
            "noop.sh", {"ATTUNE_ACTION_EXIT_CODE": "5"}
        )
        self.assertEqual(code, 5)

    def test_custom_exit_code_max(self):
        """Test maximum valid exit code (255)"""
        stdout, stderr, code = self.run_action(
            "noop.sh", {"ATTUNE_ACTION_EXIT_CODE": "255"}
        )
        self.assertEqual(code, 255)

    def test_invalid_negative_exit_code(self):
        """Test that negative exit codes are rejected"""
        stdout, stderr, code = self.run_action(
            "noop.sh", {"ATTUNE_ACTION_EXIT_CODE": "-1"}, expect_failure=True
        )
        self.assertNotEqual(code, 0)
        self.assertIn("ERROR", stderr)

    def test_invalid_large_exit_code(self):
        """Test that exit codes > 255 are rejected"""
        stdout, stderr, code = self.run_action(
            "noop.sh", {"ATTUNE_ACTION_EXIT_CODE": "999"}, expect_failure=True
        )
        self.assertNotEqual(code, 0)
        self.assertIn("ERROR", stderr)

    def test_invalid_non_numeric_exit_code(self):
        """Test that non-numeric exit codes are rejected"""
        stdout, stderr, code = self.run_action(
            "noop.sh", {"ATTUNE_ACTION_EXIT_CODE": "abc"}, expect_failure=True
        )
        self.assertNotEqual(code, 0)
        self.assertIn("ERROR", stderr)


class TestSleepAction(CorePackTestCase):
    """Tests for core.sleep action"""

    def test_basic_sleep(self):
        """Test basic sleep functionality"""
        start = time.time()
        stdout, stderr, code = self.run_action(
            "sleep.sh", {"ATTUNE_ACTION_SECONDS": "1"}
        )
        elapsed = time.time() - start

        self.assertEqual(code, 0)
        self.assertIn("Slept for 1 seconds", stdout)
        self.assertGreaterEqual(elapsed, 1.0)
        self.assertLess(elapsed, 1.5)  # Should not take too long

    def test_zero_seconds(self):
        """Test sleep with 0 seconds"""
        start = time.time()
        stdout, stderr, code = self.run_action(
            "sleep.sh", {"ATTUNE_ACTION_SECONDS": "0"}
        )
        elapsed = time.time() - start

        self.assertEqual(code, 0)
        self.assertIn("Slept for 0 seconds", stdout)
        self.assertLess(elapsed, 0.5)

    def test_sleep_with_message(self):
        """Test sleep with custom message"""
        stdout, stderr, code = self.run_action(
            "sleep.sh",
            {"ATTUNE_ACTION_SECONDS": "1", "ATTUNE_ACTION_MESSAGE": "Sleeping now..."},
        )
        self.assertEqual(code, 0)
        self.assertIn("Sleeping now...", stdout)
        self.assertIn("Slept for 1 seconds", stdout)

    def test_default_sleep_duration(self):
        """Test default sleep duration (1 second)"""
        start = time.time()
        stdout, stderr, code = self.run_action("sleep.sh", {})
        elapsed = time.time() - start

        self.assertEqual(code, 0)
        self.assertGreaterEqual(elapsed, 1.0)

    def test_invalid_negative_seconds(self):
        """Test that negative seconds are rejected"""
        stdout, stderr, code = self.run_action(
            "sleep.sh", {"ATTUNE_ACTION_SECONDS": "-1"}, expect_failure=True
        )
        self.assertNotEqual(code, 0)
        self.assertIn("ERROR", stderr)

    def test_invalid_large_seconds(self):
        """Test that seconds > 3600 are rejected"""
        stdout, stderr, code = self.run_action(
            "sleep.sh", {"ATTUNE_ACTION_SECONDS": "9999"}, expect_failure=True
        )
        self.assertNotEqual(code, 0)
        self.assertIn("ERROR", stderr)

    def test_invalid_non_numeric_seconds(self):
        """Test that non-numeric seconds are rejected"""
        stdout, stderr, code = self.run_action(
            "sleep.sh", {"ATTUNE_ACTION_SECONDS": "abc"}, expect_failure=True
        )
        self.assertNotEqual(code, 0)
        self.assertIn("ERROR", stderr)

    def test_multi_second_sleep(self):
        """Test sleep with multiple seconds"""
        start = time.time()
        stdout, stderr, code = self.run_action(
            "sleep.sh", {"ATTUNE_ACTION_SECONDS": "2"}
        )
        elapsed = time.time() - start

        self.assertEqual(code, 0)
        self.assertIn("Slept for 2 seconds", stdout)
        self.assertGreaterEqual(elapsed, 2.0)
        self.assertLess(elapsed, 2.5)


class TestHttpRequestAction(CorePackTestCase):
    """Tests for core.http_request action against an owned local server."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, _format, *_args):
                pass

            def _respond(self):
                parsed = urlparse(self.path)
                if parsed.path.startswith("/delay/"):
                    time.sleep(float(parsed.path.rsplit("/", 1)[1]))
                status = 404 if parsed.path == "/status/404" else 200
                content_length = int(self.headers.get("Content-Length", "0"))
                raw_body = self.rfile.read(content_length) if content_length else b""
                payload = {
                    "args": {key: values[-1] for key, values in parse_qs(parsed.query).items()},
                    "headers": dict(self.headers.items()),
                    "json": json.loads(raw_body) if raw_body else None,
                }
                encoded = json.dumps(payload).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(encoded)))
                self.end_headers()
                try:
                    self.wfile.write(encoded)
                except BrokenPipeError:
                    pass

            do_GET = _respond
            do_POST = _respond
            do_PUT = _respond
            do_PATCH = _respond
            do_DELETE = _respond

        cls.http_server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.http_server.daemon_threads = True
        cls.http_base_url = f"http://127.0.0.1:{cls.http_server.server_port}"
        cls.http_thread = threading.Thread(
            target=cls.http_server.serve_forever, daemon=True
        )
        cls.http_thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.http_server.shutdown()
        cls.http_server.server_close()
        cls.http_thread.join(timeout=2)
        super().tearDownClass()

    def test_simple_get_request(self):
        """Test simple GET request"""
        stdout, stderr, code = self.run_action(
            "http_request.py",
            {
                "ATTUNE_ACTION_URL": f"{self.http_base_url}/get",
                "ATTUNE_ACTION_METHOD": "GET",
            },
        )
        self.assertEqual(code, 0)

        # Parse JSON output
        result = json.loads(stdout)
        self.assertEqual(result["status_code"], 200)
        self.assertTrue(result["success"])
        self.assertIn("127.0.0.1", result["url"])

    def test_missing_url_parameter(self):
        """Test that missing URL parameter causes failure"""
        stdout, stderr, code = self.run_action(
            "http_request.py", {}, expect_failure=True
        )
        self.assertNotEqual(code, 0)
        result = json.loads(stdout)
        self.assertFalse(result["success"])
        self.assertIn("url parameter is required", result["error"])

    def test_post_with_json(self):
        """Test POST request with JSON body"""
        stdout, stderr, code = self.run_action(
            "http_request.py",
            {
                "ATTUNE_ACTION_URL": f"{self.http_base_url}/post",
                "ATTUNE_ACTION_METHOD": "POST",
                "ATTUNE_ACTION_JSON_BODY": '{"test": "value", "number": 123}',
            },
        )
        self.assertEqual(code, 0)

        result = json.loads(stdout)
        self.assertEqual(result["status_code"], 200)
        self.assertTrue(result["success"])
        # Check that our data was echoed back
        self.assertIsNotNone(result.get("json"))
        # The local fixture echoes the submitted JSON body.
        body_json = json.loads(result["body"])
        self.assertIn("json", body_json)
        self.assertEqual(body_json["json"]["test"], "value")

    def test_custom_headers(self):
        """Test request with custom headers"""
        stdout, stderr, code = self.run_action(
            "http_request.py",
            {
                "ATTUNE_ACTION_URL": f"{self.http_base_url}/headers",
                "ATTUNE_ACTION_METHOD": "GET",
                "ATTUNE_ACTION_HEADERS": '{"X-Custom-Header": "test-value"}',
            },
        )
        self.assertEqual(code, 0)

        result = json.loads(stdout)
        self.assertEqual(result["status_code"], 200)
        # The response body should contain our custom header
        body_data = json.loads(result["body"])
        self.assertIn("X-Custom-Header", body_data["headers"])

    def test_query_parameters(self):
        """Test request with query parameters"""
        stdout, stderr, code = self.run_action(
            "http_request.py",
            {
                "ATTUNE_ACTION_URL": f"{self.http_base_url}/get",
                "ATTUNE_ACTION_METHOD": "GET",
                "ATTUNE_ACTION_QUERY_PARAMS": '{"foo": "bar", "page": "1"}',
            },
        )
        self.assertEqual(code, 0)

        result = json.loads(stdout)
        self.assertEqual(result["status_code"], 200)
        # Check query params in response
        body_data = json.loads(result["body"])
        self.assertEqual(body_data["args"]["foo"], "bar")
        self.assertEqual(body_data["args"]["page"], "1")

    def test_timeout_handling(self):
        """Test request timeout"""
        stdout, stderr, code = self.run_action(
            "http_request.py",
            {
                "ATTUNE_ACTION_URL": f"{self.http_base_url}/delay/10",
                "ATTUNE_ACTION_METHOD": "GET",
                "ATTUNE_ACTION_TIMEOUT": "2",
            },
            expect_failure=True,
        )
        # Should fail due to timeout
        self.assertNotEqual(code, 0)

        result = json.loads(stdout)
        self.assertFalse(result["success"])
        self.assertIn("error", result)

    def test_404_status_code(self):
        """Test handling of 404 status"""
        stdout, stderr, code = self.run_action(
            "http_request.py",
            {
                "ATTUNE_ACTION_URL": f"{self.http_base_url}/status/404",
                "ATTUNE_ACTION_METHOD": "GET",
            },
            expect_failure=True,
        )
        # Non-2xx status codes should fail
        self.assertNotEqual(code, 0)

        result = json.loads(stdout)
        self.assertEqual(result["status_code"], 404)
        self.assertFalse(result["success"])

    def test_different_methods(self):
        """Test different HTTP methods"""
        methods = ["PUT", "PATCH", "DELETE"]

        for method in methods:
            with self.subTest(method=method):
                stdout, stderr, code = self.run_action(
                    "http_request.py",
                    {
                        "ATTUNE_ACTION_URL": f"{self.http_base_url}/{method.lower()}",
                        "ATTUNE_ACTION_METHOD": method,
                    },
                )
                self.assertEqual(code, 0)
                result = json.loads(stdout)
                self.assertEqual(result["status_code"], 200)

    def test_elapsed_time_reported(self):
        """Test that elapsed time is reported"""
        stdout, stderr, code = self.run_action(
            "http_request.py",
            {
                "ATTUNE_ACTION_URL": f"{self.http_base_url}/get",
                "ATTUNE_ACTION_METHOD": "GET",
            },
        )
        self.assertEqual(code, 0)

        result = json.loads(stdout)
        self.assertIn("elapsed_ms", result)
        self.assertIsInstance(result["elapsed_ms"], int)
        self.assertGreater(result["elapsed_ms"], 0)


class TestFilePermissions(CorePackTestCase):
    """Test that action scripts have correct permissions"""

    def test_echo_executable(self):
        """Test that echo.sh is executable"""
        script_path = self.actions_dir / "echo.sh"
        self.assertTrue(os.access(script_path, os.X_OK))

    def test_noop_executable(self):
        """Test that noop.sh is executable"""
        script_path = self.actions_dir / "noop.sh"
        self.assertTrue(os.access(script_path, os.X_OK))

    def test_sleep_executable(self):
        """Test that sleep.sh is executable"""
        script_path = self.actions_dir / "sleep.sh"
        self.assertTrue(os.access(script_path, os.X_OK))

    def test_http_request_executable(self):
        """Test that http_request.sh is executable"""
        script_path = self.actions_dir / "http_request.sh"
        self.assertTrue(os.access(script_path, os.X_OK))


class TestYAMLSchemas(CorePackTestCase):
    """Test that YAML schemas are valid"""

    def test_pack_yaml_valid(self):
        """Test that pack.yaml is valid YAML"""
        pack_yaml = self.pack_dir / "pack.yaml"
        try:
            import yaml

            with open(pack_yaml) as f:
                data = yaml.safe_load(f)
            self.assertIsNotNone(data)
            self.assertIn("ref", data)
            self.assertEqual(data["ref"], "core")
        except ImportError:
            self.skipTest("PyYAML not installed")

    def test_action_yamls_valid(self):
        """Test that all action YAML files are valid"""
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML not installed")

        for yaml_file in (self.actions_dir).glob("*.yaml"):
            with self.subTest(file=yaml_file.name):
                with open(yaml_file) as f:
                    data = yaml.safe_load(f)
                self.assertIsNotNone(data)
                self.assertIn("label", data)
                self.assertIn("ref", data)
                self.assertIn("runner_type", data)


def main():
    """Run tests"""
    # Check for pytest
    try:
        import pytest

        # Run with pytest if available
        sys.exit(pytest.main([__file__, "-v"]))
    except ImportError:
        # Fall back to unittest
        unittest.main(verbosity=2)


if __name__ == "__main__":
    main()
