#!/usr/bin/env python3
"""Diagnostic contract: owned state only; arbitrary stdout/stderr withheld."""
import contextlib
import io
import json
import unittest
from unittest.mock import patch

import workload_startup_diagnostics as diagnostics


class DiagnosticsTests(unittest.TestCase):
    def test_owned_worker_and_sensor_stderr_is_classified_without_secrets(self):
        ids = ["a" * 12, "b" * 12, "c" * 12]
        calls = []

        def command(args, ceiling=262144):
            calls.append(args)
            if args[1] == "ps":
                return "\n".join(ids), 0, False
            container = args[-1]
            if args[1] == "inspect":
                return json.dumps({
                    "project": "owned" if container != ids[2] else "neighbor",
                    "service": "worker-python" if container == ids[0] else "sensor",
                    "state": "restarting", "exit": 1, "restarts": 9, "oom": False,
                }), 0, False
            return "credential=supersecret\nworkload isolation prerequisite check failed: mount /proc: Operation not permitted\n", 0, False

        with patch.object(diagnostics, "command", side_effect=command):
            result = diagnostics.collect("owned")
        self.assertEqual([row["service"] for row in result], ["worker-python", "sensor"])
        self.assertEqual(result[0]["log_categories"], {
            "sandbox_prerequisite_failed": 1, "sandbox_proc_mount_denied": 1,
            "operation_not_permitted": 1,
        })
        self.assertNotIn("supersecret", json.dumps(result))
        self.assertFalse(any(call[1] == "logs" and call[-1] == ids[2] for call in calls))
        self.assertTrue(all(".Config.Env" not in " ".join(call) for call in calls))

    def test_unrecognized_logs_never_escape_and_overflow_is_explicit(self):
        self.assertEqual(diagnostics.classify("password=anything\nopaque decrypted context"), {})
        with patch.object(diagnostics, "command", return_value=("", 1, True)):
            self.assertEqual(diagnostics.collect("owned"), [{"diagnostic": "owned_container_listing_failed"}])

    def test_real_bounded_reader_merges_stderr_without_relaying_it(self):
        logs, code, truncated = diagnostics.command([
            "python3", "-c", "import sys;sys.stderr.write('Connection refused secret');sys.stderr.flush()",
        ], 1024)
        self.assertEqual(code, 0)
        self.assertFalse(truncated)
        self.assertEqual(diagnostics.classify(logs), {"connection_refused": 1})
        logs, _, truncated = diagnostics.command([
            "python3", "-c", "import sys;sys.stdout.write('x'*1000000)",
        ], 1024)
        self.assertEqual(len(logs), 1024)
        self.assertTrue(truncated)


if __name__ == "__main__":
    unittest.main()
