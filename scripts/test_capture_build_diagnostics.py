#!/usr/bin/env python3
"""Real first-attempt subprocess errors stay useful without leaking raw data."""
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest

TOOL = Path(__file__).resolve().with_name("capture_build_diagnostics.py")


class BuildEvidenceTests(unittest.TestCase):
    def test_compiler_failure_retains_code_location_and_exit_without_source_or_env(self):
        with tempfile.TemporaryDirectory(prefix="attune-build-evidence-") as directory:
            path = Path(directory) / "sanitized.jsonl"
            script = "import sys; print('JWT_SECRET=owned-private-credential'); print('#22 2.1 error[E0432]: unresolved import `attune_worker::runtime`',file=sys.stderr); print('#22 2.1 --> crates/api/src/lib.rs:19:7',file=sys.stderr); print('#22 2.1 --> crates/owned-private-credential.rs:1:1',file=sys.stderr); print('19 | let secret=\\\"owned-private-credential\\\";',file=sys.stderr); print('https://user:owned-private-credential@example.invalid',file=sys.stderr); sys.exit(101)"
            result = subprocess.run(["python3", str(TOOL), "--output", str(path), "--group", "initial-api", "--", "python3", "-c", script], capture_output=True, text=True)
            self.assertEqual(result.returncode, 101)
            artifact = path.read_text()
            for private in ["owned-private-credential", "JWT_SECRET", "https://", "let secret"]:
                self.assertNotIn(private, artifact + result.stdout + result.stderr)
            records = [json.loads(line) for line in artifact.splitlines()]
            self.assertTrue(any(r.get("code") == "E0432" and r["category"] == "unresolved_import" for r in records))
            self.assertTrue(any(r.get("file") == "crates/api/src/lib.rs" and r["line"] == 19 for r in records))
            self.assertEqual(records[-1]["exit_code"], 101)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_oversized_raw_line_is_drained_bounded_and_never_saved(self):
        with tempfile.TemporaryDirectory(prefix="attune-build-bounds-") as directory:
            path = Path(directory) / "sanitized.jsonl"
            result = subprocess.run(["python3", str(TOOL), "--output", str(path), "--group", "initial", "--", "python3", "-c",
                "print('owned-sensitive-' * 100000); print('error: could not compile local package'); raise SystemExit(12)"], capture_output=True, text=True)
            self.assertEqual(result.returncode, 12)
            self.assertLess(path.stat().st_size, 4096)
            self.assertNotIn("owned-sensitive", path.read_text() + result.stdout + result.stderr)
            self.assertTrue(json.loads(path.read_text().splitlines()[-1])["diagnostics_truncated"])

    def test_signal_terminates_and_joins_its_owned_child_without_retry(self):
        with tempfile.TemporaryDirectory(prefix="attune-build-signal-") as directory:
            path = Path(directory) / "sanitized.jsonl"
            marker = Path(directory) / "child-ready"
            # Child blocks on stdin until the wrapper forwards SIGTERM. The
            # wrapper emits build_start after establishing its process group.
            child = "import os,sys; from pathlib import Path; Path(sys.argv[1]).write_text(str(os.getpid())); sys.stdin.read()"
            process = subprocess.Popen(["python3", str(TOOL), "--output", str(path), "--group", "initial", "--", "python3", "-c", child, str(marker)],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                self.assertIn("build_start", process.stdout.readline())
                # Popen has established the process group before build_start.
                process.send_signal(signal.SIGTERM)
                stdout, stderr = process.communicate(timeout=15)
                self.assertEqual(process.returncode, 143, stdout + stderr)
                self.assertEqual(json.loads(path.read_text().splitlines()[-1])["exit_code"], 143)
                if marker.exists():
                    with self.assertRaises(ProcessLookupError):
                        os.kill(int(marker.read_text()), 0)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=15)


if __name__ == "__main__":
    unittest.main()
