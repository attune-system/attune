"""Execute real entrypoints with owned local tools; no network credentials."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ACTIONS = Path(__file__).resolve().parents[1] / "actions"


class TestStdinJson(unittest.TestCase):
    def run_action(self, action, params, env=None):
        return subprocess.run(
            ["/bin/sh", str(ACTIONS / f"{action}.sh")],
            input=json.dumps(params), text=True, capture_output=True,
            env=env or os.environ.copy(), timeout=10,
        )

    def test_message_cannot_override_other_parameters(self):
        message = "'quoted' \\ unicode 雪\nexit_code=9\nseconds=3600\nmessage=changed\n"
        result = self.run_action("echo", {"message": message})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, message)
        result = self.run_action("noop", {"message": message, "exit_code": 0})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(message, result.stdout)
        result = self.run_action("sleep", {"message": message, "seconds": 0})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(message, result.stdout)

    def test_versioned_python_without_unversioned_alias(self):
        for name in ("python3.11", "python3.12", "python3.13", "python"):
            with self.subTest(interpreter=name), tempfile.TemporaryDirectory() as directory:
                # This owned PATH has one runnable interpreter, no python3 alias
                # and no external shell utilities. Python checks its real version.
                (Path(directory) / name).symlink_to(sys.executable)
                env = {**os.environ, "PATH": directory,
                       "ATTUNE_API_TOKEN": "dummy", "ATTUNE_API_URL": "http://unused.invalid"}
                cases = {
                    "echo": {"message": "versioned interpreter\n"},
                    "noop": {}, "sleep": {"seconds": 0},
                    "run_agent_command": {"command": "printf reached", "require_mcp_binary": False},
                    "http_request": {},
                }
                for action, params in cases.items():
                    result = self.run_action(action, params, env)
                    self.assertEqual(result.returncode, 1 if action == "http_request" else 0,
                                     (action, result.stdout, result.stderr))
                    if action == "echo":
                        self.assertEqual(result.stdout, params["message"])
                    if action == "http_request":
                        self.assertEqual(json.loads(result.stdout)["error"], "url parameter is required")

    def test_no_suitable_python_fails_before_reading_parameters(self):
        with tempfile.TemporaryDirectory() as directory:
            env = {**os.environ, "PATH": directory}
            result = self.run_action("echo", {"message": "private value"}, env)
            self.assertEqual(result.returncode, 126)
            self.assertEqual(result.stdout, "")
            self.assertIn("runnable Python", result.stderr)
            self.assertNotIn("private value", result.stderr)

    def test_agent_command_is_fixed_despite_multiline_state(self):
        with tempfile.TemporaryDirectory() as directory:
            state = str(Path(directory) / "雪\ncommand=printf injected\n")
            env = {**os.environ, "ATTUNE_API_TOKEN": "dummy-owned-token", "ATTUNE_API_URL": "http://unused.invalid"}
            command = "python3 -c 'import os,json; print(json.dumps({k:os.environ[k] for k in [\"ATTUNE_AGENT_STATE_DIR\",\"ATTUNE_MCP_COMMAND\",\"ATTUNE_MCP_TRANSPORT\"]}))'"
            params = dict(command=command, state_dir=state, require_mcp_binary=False,
                          mcp_command="/unused/quoted ' path\n雪")
            result = self.run_action("run_agent_command", params, env)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), dict(ATTUNE_AGENT_STATE_DIR=state,
                             ATTUNE_MCP_COMMAND=params["mcp_command"], ATTUNE_MCP_TRANSPORT="stdio"))
            self.assertTrue(Path(state).is_dir())
            for missing in ("ATTUNE_API_TOKEN", "ATTUNE_API_URL"):
                denied_env = env.copy()
                denied_env.pop(missing)
                denied = self.run_action("run_agent_command", params, denied_env)
                self.assertNotEqual(denied.returncode, 0)
                self.assertEqual(denied.stdout, "")
            params["require_mcp_binary"] = True
            self.assertNotEqual(self.run_action("run_agent_command", params, env).returncode, 0)

    def test_curl_argv_and_body_are_not_line_encoded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = root / "curl"
            fake.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
body = pathlib.Path(args[args.index('--data-binary') + 1][1:]).read_bytes().decode()
pathlib.Path(os.environ['CAPTURE']).write_text(json.dumps(dict(args=args, body=body)))
pathlib.Path(args[args.index('--output') + 1]).write_text('"response 雪\\\\n"\\n')
pathlib.Path(args[args.index('--dump-header') + 1]).write_text('HTTP/1.1 200 OK\\nX-Test: value\\n\\n')
print('200\\nhttp://local/"quoted"', end='')
''')
            fake.chmod(0o700)
            env = {**os.environ, "PATH": directory + os.pathsep + os.environ["PATH"], "CAPTURE": str(root / "capture")}
            header = "quotes '\" 雪\n--output\n/should-not-write"
            body = "@/not-a-file\nquotes '\" \\ 雪\n\n"
            for params in ({"body": body}, {"json_body": {"message": body}}):
                result = self.run_action("http_request", {**params, "url": "--output\n/should-not-write",
                        "headers": {"X-Test": header}, "auth_type": "basic",
                        "auth_username": "user", "auth_password": "pass\n--insecure"}, env)
                self.assertEqual(result.returncode, 0, result.stderr)
                captured = json.loads((root / "capture").read_text())
                args = captured["args"]
                self.assertEqual(args[-2:], ["--url", "--output\n/should-not-write"])
                self.assertIn("X-Test: " + header, args)
                self.assertIn("user:pass\n--insecure", args)
                self.assertEqual(args.count("--output"), 1)
                self.assertEqual(args[args.index("--proto") + 1], "=http,https")
                self.assertEqual(args[args.index("--proto-redir") + 1], "=http,https")
                if "json_body" in params:
                    self.assertEqual(json.loads(captured["body"]), params["json_body"])
                else:
                    self.assertEqual(captured["body"], body)
                output = json.loads(result.stdout)
                self.assertEqual(output["url"], 'http://local/"quoted"')
                self.assertEqual(output["body"], '"response 雪\\n"\n')

    def test_non_http_protocols_rejected_by_real_curl(self):
        for url in ("file:///etc/passwd", "ftp://127.0.0.1/unused"):
            with self.subTest(url=url):
                result = self.run_action("http_request", {"url": url})
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(json.loads(result.stdout)["success"])

    def test_old_lines_and_non_object_json_fail_closed(self):
        for action in ("echo", "noop", "sleep", "run_agent_command", "http_request"):
            for payload in ('message=first\ncommand=changed\n', '[]'):
                result = subprocess.run(["/bin/sh", str(ACTIONS / f"{action}.sh")],
                                        input=payload, text=True, capture_output=True, timeout=10)
                self.assertNotEqual(result.returncode, 0, (action, result.stdout))
