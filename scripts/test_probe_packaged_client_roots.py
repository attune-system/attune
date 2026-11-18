"""Pure wire/result checks; actual image initialization is owned E2E evidence."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import unittest

SPEC = importlib.util.spec_from_file_location('client_roots', Path(__file__).with_name('probe_packaged_client_roots.py'))
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)


class PackagedClientRootsTests(unittest.TestCase):
    def test_real_remote_commands_and_empty_store_controls(self):
        calls = []

        def run(args, **kwargs):
            calls.append(args)
            self.assertEqual(kwargs, dict(capture_output=True, timeout=30, check=False))
            binary = args[args.index('--entrypoint') + 1].rsplit('/', 1)[1]
            if 'SSL_CERT_FILE=/dev/null' in args:
                self.assertIn('SSL_CERT_DIR=/attune-owned-probe-no-certificates', args)
                return subprocess.CompletedProcess(args, 1, b'', probe.MISSING_ROOTS)
            data = {'local': {'binary': binary, 'version': 'candidate'},
                    'server': {'status': 'connected', 'version': 'candidate'}}
            return subprocess.CompletedProcess(args, 0, json.dumps(data).encode(), b'')

        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(probe.probe(['docker', 'compose', '-p', 'owned'], run), 0)
        self.assertEqual(len(calls), 4)
        self.assertTrue(all('--local' not in args and '--rm' in args and '--no-deps' in args for args in calls))
        self.assertTrue(all(item['passed'] for item in map(json.loads, output.getvalue().splitlines())))

    def test_wrong_failure_timeout_and_unavailable_are_not_initialization_evidence(self):
        for result in (
            subprocess.CompletedProcess([], 1, b'', b'fixture-private-value'),
            subprocess.CompletedProcess([], 0, b'{"server":{"status":"unavailable"}}', b''),
            subprocess.CompletedProcess([], 0, b'not JSON', b''),
        ):
            self.assertFalse(probe.check(result, 'attune', False))
            self.assertFalse(probe.check(result, 'attune', True))
        with contextlib.redirect_stdout(io.StringIO()) as output:
            def timeout(*args, **kwargs):
                raise subprocess.TimeoutExpired('fixture-private-command', 30, b'fixture-private-output')
            self.assertEqual(probe.probe(['docker', 'compose'], timeout), 1)
        self.assertNotIn('fixture-private', output.getvalue())
        self.assertFalse(json.loads(output.getvalue())['passed'])


if __name__ == '__main__':
    unittest.main()
