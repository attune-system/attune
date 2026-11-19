"""Mocked full runner lifecycle; inert socket inode, synthetic ZIP, no services."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location('native_inputs', ROOT / 'scripts/test_prepare_native_compose.py')
inputs = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(inputs)


class NativeRunner(unittest.TestCase):
    def test_owned_lifecycle_keeps_all_profiles_and_coverage(self):
        for cleanup_status in (0, 9):
            with self.subTest(cleanup_status=cleanup_status), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                profiles = inputs.Preparation().inputs(root)
                (root / 'profiles.json').write_text(json.dumps(profiles))
                # Filesystem-only socket type, not a bound socket or connection.
                os.mknod(root / 'engine.sock', stat.S_IFSOCK | 0o600)
                owner = 'native-' + root.name.replace('_', '-')
                events = root / 'events'
                docker = root / 'docker'
                docker.write_text('#!' + sys.executable + '\n' + r'''
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ['EVENTS'], 'a') as f:
    f.write(json.dumps({'args': args, 'host': os.environ.get('DOCKER_HOST'), 'context': os.environ.get('DOCKER_CONTEXT')}) + '\n')
if args[:1] == ['--host']:
    args = args[2:]
if args[0] == 'info':
    print(json.dumps({'ID':'fixture-observed-daemon','OSType':'linux','Architecture':'amd64'}))
elif args[:2] == ['volume','inspect']:
    print(json.dumps([{'Name':args[2],'Driver':'local','Options':None,'Labels':{'io.attune.test.owner':os.environ['ATTUNE_E2E_RUN_ID']}}]))
elif args[:2] == ['network','inspect']:
    print(json.dumps([{'Name':args[2],'Driver':'bridge','Containers':{},'Labels':{'io.attune.test.owner':os.environ['ATTUNE_E2E_RUN_ID']}}]))
elif args[:2] == ['image','inspect']:
    print(json.dumps([{'Id':args[2],'Os':'linux','Architecture':'amd64'}]))
elif args[0] in ('ps','volume','network'):
    pass
elif args[0] == 'inspect':
    print('healthy')
elif args[0] == 'compose':
    args = args[1:]
    while args and args[0] in ('--project-directory','--project-name','-f'):
        args = args[2:]
    if args[:2] == ['ps','-q']: print('owned-fixture-container')
    elif args[0] == 'exec': print('1')
    elif args[0] == 'down': sys.exit(int(os.environ['CLEANUP_STATUS']))
    elif args[0] not in ('ps','up','wait','run'): sys.exit(97)
else:
    sys.exit(98)
''')
                docker.chmod(0o755)
                python = root / 'python3'
                # Packaged probe behavior has its own tests. This test verifies
                # invocation/ordering without starting its actual child clients.
                python.write_text('#!' + sys.executable + '\n' + r'''
import os, sys
if sys.argv[1].endswith('probe_packaged_client_roots.py'):
    with open(os.environ['PROBE'], 'w') as f: f.write('invoked')
else:
    os.execv(sys.executable, [sys.executable, *sys.argv[1:]])
''')
                python.chmod(0o755)
                env = dict(os.environ, PATH=str(root) + ':' + os.environ['PATH'], EVENTS=str(events),
                           PROBE=str(root / 'probe'), CLEANUP_STATUS=str(cleanup_status),
                           ATTUNE_E2E_RUN_ID=owner, ATTUNE_TEST_RUN_ID='native-render',
                           ATTUNE_E2E_DOCKER_SOCKET=str(root / 'engine.sock'),
                           ATTUNE_BOOTSTRAP_TOKEN='synthetic-not-a-token',
                           ATTUNE_E2E_RETENTION_PROFILE='default', DOCKER_CONTEXT='must-not-be-used',
                           DOCKER_HOST='unix:///must-not-be-used')
                for name in ('ATTUNE_E2E_PROJECT_NAME', 'ATTUNE_E2E_COMPOSE_OVERRIDE', 'ATTUNE_DOCKER_CONFIG_PATH'):
                    env.pop(name, None)
                native_root = None
                try:
                    result = subprocess.run(['bash', str(ROOT / 'scripts/run-integration-tests.sh'),
                                             '--native-profiles', str(root / 'profiles.json'),
                                             '--runtime-lifecycle', '--tier', '1', '-k', 'original-selection'],
                                            env=env, text=True, capture_output=True, timeout=30)
                    calls = [json.loads(line) for line in events.read_text().splitlines()]
                    compose_calls = [c['args'] for c in calls if c['args'][0] == 'compose']
                    generated = next(arg for call in compose_calls for arg in call if arg.endswith('/config/compose.yaml'))
                    native_root = Path(generated).parent.parent
                    self.assertEqual(result.returncode, cleanup_status, result.stdout + result.stderr)
                    self.assertEqual((root / 'probe').read_text(), 'invoked')
                    self.assertTrue(all(c['host'] == 'unix://' + str(root / 'engine.sock') and c['context'] is None for c in calls))
                    self.assertTrue(all('--project-directory' in c for c in compose_calls))
                    startup = next(c for c in compose_calls if 'worker-toolchains' in c)
                    for service in inputs.module.MANAGERS:
                        self.assertIn(service, startup)
                    # E2E resets precede run-owned toolchain reset and lifecycle admissions.
                    files = [startup[i + 1] for i, arg in enumerate(startup) if arg == '-f']
                    self.assertEqual(files[0], generated)
                    self.assertEqual(files[1], str(ROOT / 'docker-compose.e2e.yaml'))
                    self.assertTrue(files[-1].endswith('/fixtures/compose.yaml'))
                    self.assertIn('container_name: !reset null', (native_root / 'toolchains-e2e.yaml').read_text())
                    registrations = [c[-1] for c in compose_calls if 'exec' in c]
                    for name in ('worker-shell','worker-python','worker-node','worker-full','worker-toolchains','sensor-primary','sensor-secondary'):
                        self.assertTrue(any("name = '" + name + "-01'" in sql for sql in registrations))
                    tests = next(c for c in compose_calls if 'original-selection' in c)
                    self.assertEqual(tests[-6:], ['--tier', '1', '-k', 'original-selection', '-m', 'not performance'])
                    self.assertTrue(all('--no-build' in c and c[c.index('--pull') + 1] == 'never'
                                        for c in compose_calls if 'up' in c or 'run' in c))
                    self.assertFalse(any(c['args'][0] in ('create','rm','stop') or 'build' in c['args'] for c in calls))
                    self.assertTrue(native_root.exists())
                    down = next(c for c in compose_calls if 'down' in c)
                    self.assertNotIn('--volumes', down)
                    self.assertIn('receipt-aware cleanup', result.stdout)
                finally:
                    if native_root:
                        shutil.rmtree(native_root)
                    # Remove only the exact synthetic runtime fixture directories
                    # allocated by this mock run (retained on failed teardown).
                    for path in (ROOT / '.scratch').glob('runtime-service-' + owner + '.*'):
                        shutil.rmtree(path)

    def test_native_requires_explicit_socket_and_disallows_unadmitted_attach(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            docker = root / 'docker'
            docker.write_text('#!/bin/sh\nexit 99\n')
            docker.chmod(0o755)
            env = dict(os.environ, PATH=str(root) + ':' + os.environ['PATH'], ATTUNE_E2E_PROJECT_NAME='explicit-project')
            env.pop('ATTUNE_E2E_DOCKER_SOCKET', None)
            for options in ([], ['--standalone'], ['--no-startup', '--no-build']):
                result = subprocess.run(['bash', str(ROOT / 'scripts/run-integration-tests.sh'),
                                         '--native-profiles', str(root / 'missing.json'), *options],
                                        env=env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 2)


if __name__ == '__main__':
    unittest.main()
