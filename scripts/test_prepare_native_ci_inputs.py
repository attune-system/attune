"""Pure CI input/runner tests. All engine effects are controlled doubles."""
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location('native_ci', ROOT / 'scripts/prepare-native-ci-inputs.py')
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)
OWNER = 'ci-123-2'
NAMESPACE = dict(engine_id=13, helper_pin_id=17, helper_object_id=19,
                 object_version='ci:' + 'd' * 40 + ':123:2')


def runner_bytes(name='linux-amd64', mode=0o755):
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w') as archive:
        entry = zipfile.ZipInfo(name)
        entry.external_attr = mode << 16
        archive.writestr(entry, b'synthetic-public-runner-not-an-executable')
    return out.getvalue()


class FakeDocker:
    """Records effects; never invokes a daemon, process, socket or provider."""
    def __init__(self, root, fault=None):
        self.root, self.fault, self.calls = root, fault, []
        self.volumes = {}
        self.container = None

    def json(self, *args):
        return json.loads(self.run(*args))

    def run(self, *args):
        self.calls.append(args)
        if args[0] == 'info':
            return json.dumps(dict(ID='synthetic-observed-daemon', OSType='linux', Architecture='amd64'))
        if args[:2] == ('image', 'inspect'):
            arch = 'arm64' if self.fault == 'image-platform' else 'amd64'
            return json.dumps([dict(Id='sha256:' + hashlib.sha256(args[2].encode()).hexdigest(), Os='linux', Architecture=arch)])
        if args[0] in ('volume', 'network') and args[1] == 'ls':
            if self.fault == 'listing-unknown':
                raise ValueError('synthetic transport uncertainty')
            return ''
        if args[:2] == ('network', 'create'):
            self.network = args[-1]
            self.labels = dict(args[i + 1].split('=', 1) for i, a in enumerate(args) if a == '--label')
            return 'b' * 64
        if args[:2] == ('network', 'inspect'):
            result = dict(Name=self.network, Id='b' * 64, Labels=self.labels, Driver='bridge', Containers={})
            if self.fault == 'network-unknown':
                result.pop('Containers')
            return json.dumps([result])
        if args[:2] == ('volume', 'create'):
            self.volumes[args[-1]] = dict(Name=args[-1], Labels=self.labels, Driver='local', Options=None)
            return args[-1]
        if args[:2] == ('volume', 'inspect'):
            value = copy.deepcopy(self.volumes[args[2]])
            if self.fault == 'volume-options':
                value['Options'] = {'device': '/unowned'}
            return json.dumps([value])
        if args[0] == 'ps':
            return '{"ID":"unowned-user"}' if self.fault == 'volume-user' else ''
        if args[0] == 'create':
            if self.fault == 'create-unknown':
                raise ValueError('synthetic ambiguous create ACK')
            image = args[args.index('--entrypoint') + 2]
            mounts = []
            for i, arg in enumerate(args):
                if arg == '--mount':
                    fields = dict(v.split('=', 1) for v in args[i + 1].split(','))
                    mounts.append(dict(Type=fields['type'], Destination=fields['dst'],
                                       **{('Name' if fields['type'] == 'volume' else 'Source'): fields['src']}))
            self.container = dict(Id='c' * 64, Image=image, Mounts=mounts,
                                  Config=dict(Labels=self.labels, Entrypoint=['/bin/sh'], Cmd=list(args[-2:])),
                                  HostConfig=dict(NetworkMode='none', ReadonlyRootfs=True, Privileged=False, Memory=536870912, PidsLimit=128),
                                  State=dict(Running=False, Status='created', ExitCode=0, OOMKilled=False, Error=''))
            if self.fault == 'command-substitution':
                self.container['Config']['Cmd'] = ['-ec', 'untrusted']
            return 'c' * 64
        if args[0] == 'inspect':
            value = copy.deepcopy(self.container)
            if self.fault == 'state-unknown' and value['State']['Status'] == 'exited':
                value['State'].pop('Running')
            return json.dumps([value])
        if args[0] == 'start':
            # Simulated output of the fixed initializer, NOT a real helper run.
            if self.fault != 'missing-capture':
                (self.root / 'runner.zip').write_bytes(runner_bytes())
                (self.root / 'runner.zip').chmod(0o444)
            self.container['State']['Status'] = 'exited'
            return 'c' * 64
        if args[0] == 'wait':
            return '9' if self.fault == 'negative-exit' else '0'
        if args[0] == 'rm':
            assert self.container['State']['Status'] == 'exited'
            return 'wrong' if self.fault == 'removal-unknown' else 'c' * 64
        raise AssertionError('unexpected effect: ' + repr(args))


def modeled_owners(function):
    """Synthetic files model manager1000 ownership even on CI's host UID1001."""
    def read(path, *args, **kwargs):
        meta = function(path, *args, **kwargs)
        if path.name == 'runner.zip':
            fields = {name: getattr(meta, name) for name in ('st_mode', 'st_nlink', 'st_size')}
            return SimpleNamespace(**fields, st_uid=1000)
        return meta
    return read


class Preparation(unittest.TestCase):
    def planned(self, root):
        supplied = root / 'operator.json'
        supplied.write_text(json.dumps(NAMESPACE))
        module.plan(root, 'owned-ci-project', OWNER, supplied)

    def prepare(self, root, docker):
        with patch.object(module.Path, 'lstat', modeled_owners(module.Path.lstat)):
            return module.prepare(root, Path('/explicit/inert-socket'), docker)

    def test_plan_is_static_and_requires_explicit_positive_run_source_namespace(self):
        for key, value in [('engine_id', 0), ('engine_id', 2**63), ('helper_pin_id', True), ('helper_object_id', None),
                           ('object_version', 'x' * 129), ('object_version', 'é'),
                           ('object_version', 'published-release'), ('object_version', 'ci:' + 'd'*40 + ':123:1')]:
            with self.subTest(key=key, value=value), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                namespace = dict(NAMESPACE, **{key: value})
                supplied = root / 'operator.json'
                supplied.write_text(json.dumps(namespace))
                with patch.object(module.subprocess, 'run', side_effect=AssertionError('no process in plan')):
                    with self.assertRaises(ValueError):
                        module.plan(root, 'owned-ci', OWNER, supplied)
                self.assertFalse((root / 'plan.json').exists())

    def test_same_candidate_bytes_and_operator_ids_all_seven_private_profiles(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.planned(root)
            docker = FakeDocker(root)
            profiles = self.prepare(root, docker)
            self.assertEqual(set(profiles), set(module.compose.MANAGERS))
            volumes = []
            for service, profile in profiles.items():
                workload = profile['workload']
                self.assertEqual(workload['backend']['binding'], dict(engine_id=13, daemon_id='synthetic-observed-daemon', platform='linux/amd64'))
                self.assertEqual((workload['memory_bytes'], workload['scratch_bytes']), (536870912, 134217728))
                self.assertEqual(workload['storage'], dict(compressed_bytes=67108864, expanded_bytes=134217728, entries=4096))
                self.assertEqual(workload['backend']['pids_limit'], 128)
                pin = workload['helper']
                self.assertEqual((pin['pin_id'], pin['object_id'], pin['object_version']), (17, 19, NAMESPACE['object_version']))
                self.assertEqual(pin['sha256'], hashlib.sha256(runner_bytes()).hexdigest())
                self.assertEqual(pin['size_bytes'], len(runner_bytes()))
                self.assertEqual(pin['max_files'], 1)
                volumes.extend([workload['backend']['cache_volume'], profile['custody_volume']])
            self.assertEqual(len(set(volumes)), 14)
            creates = [a for a in docker.calls if a[0] == 'create']
            self.assertEqual(len(creates), 1)
            create = creates[0]
            self.assertIn('--read-only', create)
            self.assertEqual(create[create.index('--network') + 1], 'none')
            self.assertNotIn('--env', create)
            self.assertNotIn('docker.sock', ' '.join(create))
            self.assertIn('chown 1000:1000', create[-1])
            self.assertIn('chmod 0700', create[-1])
            self.assertIn('chown 0:0 /ci-input/runner.zip', create[-1])
            self.assertEqual(docker.calls[-1], ('rm', 'c' * 64))
            state = json.loads((root / 'observations.json').read_text())
            self.assertTrue(state['initializer_removed'])
            self.assertEqual(state['namespace'], NAMESPACE)
            self.assertFalse(any(a[0] in ('build', 'pull', 'exec') for a in docker.calls))

    def test_unknowns_and_negative_facts_retain_evidence_without_cleanup_or_profiles(self):
        for fault in ('image-platform', 'listing-unknown', 'network-unknown', 'volume-options',
                      'volume-user', 'create-unknown', 'command-substitution', 'negative-exit',
                      'state-unknown', 'missing-capture', 'removal-unknown'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.planned(root)
                docker = FakeDocker(root, fault)
                with self.assertRaises((ValueError, FileNotFoundError)):
                    self.prepare(root, docker)
                self.assertTrue((root / 'observations.json').exists())
                self.assertTrue((root / 'admission.json').exists())
                self.assertFalse((root / 'profiles.json').exists())
                self.assertFalse(any(a[0] in ('stop', 'kill') or (a[0] in ('volume', 'network') and a[1] == 'rm') for a in docker.calls))
                if fault != 'removal-unknown':
                    self.assertFalse(any(a[0] == 'rm' for a in docker.calls))

    def test_runner_owner_links_modes_platform_and_inventory_are_actual_facts(self):
        for bad in ('owner', 'links', 'writable', 'symlink', 'platform', 'entry-link', 'entry-mode', 'duplicate'):
            with self.subTest(bad=bad), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                runner = root / 'runner.zip'
                runner.write_bytes(runner_bytes('linux-arm64' if bad == 'platform' else 'linux-amd64',
                                                stat.S_IFLNK | 0o755 if bad == 'entry-link' else (0o644 if bad == 'entry-mode' else 0o755)))
                if bad == 'duplicate':
                    with self.assertWarns(UserWarning), zipfile.ZipFile(runner, 'a') as archive:
                        archive.writestr('linux-amd64', b'duplicate')
                runner.chmod(0o666 if bad == 'writable' else 0o444)
                if bad == 'links':
                    os.link(runner, root / 'alias')
                if bad == 'symlink':
                    runner.rename(root / 'body')
                    runner.symlink_to(root / 'body')
                modeled = modeled_owners(module.Path.lstat)
                def metadata(path, *args, **kwargs):
                    value = modeled(path, *args, **kwargs)
                    if bad == 'owner' and path == runner:
                        value.st_uid = 1001
                    return value
                with patch.object(module.Path, 'lstat', metadata):
                    with self.assertRaises((ValueError, KeyError)):
                        module.runner_facts(runner, 'linux/amd64')

    def test_endpoint_selection_and_subprocess_environment_have_no_context_fallback(self):
        with patch.object(module.Path, 'is_symlink', return_value=False), patch.object(module.Path, 'stat', return_value=SimpleNamespace(st_mode=stat.S_IFSOCK)):
            docker = module.Docker(Path('/explicit/inert-socket'))
        with patch.object(module.subprocess, 'run', return_value=SimpleNamespace(returncode=0, stdout='ack\n')) as run:
            self.assertEqual(docker.run('wait', 'c'*64), 'ack')
        self.assertEqual(run.call_args.args[0][:3], ['docker', '--host', 'unix:///explicit/inert-socket'])
        self.assertEqual(set(run.call_args.kwargs['env']), {'PATH'})


class Runner(unittest.TestCase):
    def test_invalid_native_ci_modes_fail_before_any_engine_command(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            supplied = root / 'admission.json'
            supplied.write_text(json.dumps(NAMESPACE))
            docker = root / 'docker'
            docker.write_text('#!/bin/sh\nexit 99\n')
            docker.chmod(0o755)
            env = dict(os.environ, PATH=str(root) + ':' + os.environ['PATH'],
                       ATTUNE_E2E_PROJECT_NAME='explicit-project')
            env.pop('ATTUNE_E2E_DOCKER_SOCKET', None)
            for options in ([], ['--no-build'], ['--standalone'], ['--no-startup', '--no-build'],
                            ['--native-profiles', str(supplied)]):
                result = subprocess.run(['bash', str(ROOT / 'scripts/run-integration-tests.sh'),
                                         '--native-ci-admission', str(supplied), *options],
                                        env=env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 2)

    def test_one_captured_build_then_native_inputs_before_any_start_and_failure_retention(self):
        for fault in ('none', 'build', 'dependency-pull', 'create-unknown'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                supplied = root / 'admission.json'
                supplied.write_text(json.dumps(NAMESPACE))
                # Inert socket metadata only; every CLI call below is a double.
                os.mknod(root / 'inert.sock', stat.S_IFSOCK | 0o600)
                docker = root / 'docker'
                docker.write_text('#!' + sys.executable + '\n' + r'''
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ['EVENTS'], 'a') as file:
    file.write(json.dumps({'args':args,'host':os.environ.get('DOCKER_HOST'),'context':os.environ.get('DOCKER_CONTEXT')})+'\n')
if args[:1] == ['--host']: args = args[2:]
state = {}
record = pathlib.Path(os.environ['CI_ROOT_RECORD'])
if record.exists():
    observed = pathlib.Path(record.read_text()) / 'observations.json'
    if observed.exists(): state = json.loads(observed.read_text())
if args[0] == 'compose':
    args = args[1:]
    while args and args[0] in ('--project-directory','--project-name','--progress','-f'): args = args[2:]
    if args[0] == 'build':
        print('#1 DONE 0.0s')
        sys.exit(8 if os.environ['FAULT'] == 'build' else 0)
    elif args[0] == 'pull':
        if args[1:] != ['postgres','rabbitmq','init-packs']: sys.exit(96)
        if os.environ['FAULT'] == 'dependency-pull': sys.exit(9)
        record.with_suffix('.acquired').write_text('image-only-dependencies')
    elif args[0] in ('up','run'):
        # A fresh endpoint has no implicit infrastructure image cache.
        if not record.with_suffix('.acquired').exists(): sys.exit(95)
    elif args[:2] == ['ps','-q']: print('owned-fixture-container')
    elif args[0] == 'exec': print('1')
    elif args[0] not in ('ps','up','wait','run','down'): sys.exit(97)
elif args[0] == 'info':
    print(json.dumps({'ID':'synthetic-observed-daemon','OSType':'linux','Architecture':'amd64'}))
elif args[:2] == ['volume','inspect']:
    print(json.dumps([{'Name':args[2],'Driver':'local','Options':None,'Labels':{'io.attune.test.owner':os.environ['ATTUNE_E2E_RUN_ID']}}]))
elif args[:2] == ['network','inspect']:
    print(json.dumps([{'Name':args[2],'Driver':'bridge','Containers':{},'Labels':{'io.attune.test.owner':os.environ['ATTUNE_E2E_RUN_ID']}}]))
elif args[:2] == ['image','inspect']:
    print(json.dumps([{'Id':args[2],'Os':'linux','Architecture':'amd64'}]))
elif args[0] == 'inspect': print('healthy')
elif args[0] not in ('ps','volume','network'): sys.exit(98)
''')
                docker.chmod(0o755)
                python = root / 'python3'
                python.write_text('#!' + sys.executable + '\n' + r'''
import importlib.util, json, os, pathlib, sys
script, args = sys.argv[1], sys.argv[2:]
if script.endswith('prepare-native-ci-inputs.py'):
    phase = 'plan' if '--plan' in args else 'prepare'
    target = pathlib.Path(args[args.index('--root')+1])
    pathlib.Path(os.environ['CI_ROOT_RECORD']).write_text(str(target))
    with open(os.environ['EVENTS'],'a') as f: f.write(json.dumps({'phase':phase})+'\n')
    if phase == 'prepare':
        spec = importlib.util.spec_from_file_location('ci_tests', pathlib.Path(script).with_name('test_prepare_native_ci_inputs.py'))
        tests = importlib.util.module_from_spec(spec); spec.loader.exec_module(tests)
        tests.Preparation().prepare(target, tests.FakeDocker(target, os.environ['FAULT']))
        raise SystemExit(0)
elif script.endswith('probe_packaged_client_roots.py'):
    pathlib.Path(os.environ['PROBE']).write_text('invoked')
    raise SystemExit(0)
os.execv(sys.executable, [sys.executable, script, *args])
''')
                python.chmod(0o755)
                events, record = root / 'events', root / 'ci-root'
                env = dict(os.environ, PATH=str(root) + ':' + os.environ['PATH'], EVENTS=str(events),
                           CI_ROOT_RECORD=str(record), PROBE=str(root / 'probe'), FAULT=fault,
                           ATTUNE_E2E_RUN_ID=OWNER, ATTUNE_TEST_RUN_ID='native-ci-pure',
                           ATTUNE_E2E_DOCKER_SOCKET=str(root / 'inert.sock'),
                           ATTUNE_BOOTSTRAP_TOKEN='synthetic-not-a-token',
                           ATTUNE_E2E_RETENTION_PROFILE='default', DOCKER_CONTEXT='must-not-select',
                           DOCKER_HOST='unix:///must-not-select')
                for name in ('ATTUNE_E2E_PROJECT_NAME', 'ATTUNE_E2E_COMPOSE_OVERRIDE',
                             'ATTUNE_DOCKER_CONFIG_PATH', 'ATTUNE_E2E_SERIAL_BUILD'):
                    env.pop(name, None)
                diagnostics = ROOT / '.scratch/e2e-diagnostics'
                before = set(diagnostics.glob(OWNER + '.*')) if diagnostics.exists() else set()
                calls = []
                try:
                    result = subprocess.run(['bash', str(ROOT / 'scripts/run-integration-tests.sh'),
                                             '--native-ci-admission', str(supplied), '--runtime-lifecycle',
                                             '--tier', '1', '-k', 'original-selection'], env=env,
                                            capture_output=True, text=True, timeout=30)
                    calls = [json.loads(line) for line in events.read_text().splitlines()]
                    cli = [c for c in calls if 'args' in c]
                    builds = [c for c in cli if 'build' in c['args']]
                    self.assertEqual(len(builds), 1, result.stdout + result.stderr)
                    self.assertIn('init-agent', builds[0]['args'])
                    for service in module.BUILD_MANAGERS:
                        self.assertIn(service, builds[0]['args'])
                    self.assertTrue(all(c['host'] == 'unix://' + str(root / 'inert.sock') and c['context'] is None for c in cli))
                    inputs = Path(record.read_text())
                    self.assertTrue((inputs / 'admission.json').exists())
                    pulls = [c for c in cli if 'pull' in c['args'] and '--pull' not in c['args']]
                    self.assertEqual(len(pulls), 0 if fault == 'build' else 1)
                    if pulls:
                        self.assertEqual(pulls[0]['args'][-4:], ['pull','postgres','rabbitmq','init-packs'])
                        self.assertLess(calls.index(builds[0]), calls.index(pulls[0]))
                    if fault != 'none':
                        self.assertNotEqual(result.returncode, 0)
                        if fault == 'build':
                            self.assertEqual(result.returncode, 8)
                        if fault == 'dependency-pull':
                            self.assertEqual(result.returncode, 9)
                            self.assertFalse(any(c.get('phase') == 'prepare' for c in calls))
                        self.assertFalse(any('up' in c['args'] for c in cli))
                        self.assertFalse((inputs / 'profiles.json').exists())
                        self.assertIn('retained', result.stdout)
                        continue
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    prepared = next(i for i,c in enumerate(calls) if c.get('phase') == 'prepare')
                    first_start = next(i for i,c in enumerate(calls) if 'up' in c.get('args', []))
                    self.assertLess(calls.index(builds[0]), prepared)
                    self.assertLess(calls.index(pulls[0]), prepared)
                    self.assertLess(prepared, first_start)
                    startup = next(c['args'] for c in cli if 'up' in c['args'] and 'worker-toolchains' in c['args'])
                    for service in module.compose.MANAGERS:
                        self.assertIn(service, startup)
                    files = [startup[i+1] for i,a in enumerate(startup) if a == '-f']
                    self.assertEqual(files[0], str(inputs / 'config/compose.yaml'))
                    self.assertEqual(files[-2], str(inputs / 'launch-images.json'))
                    self.assertTrue(files[-1].endswith('/fixtures/compose.yaml'))
                    self.assertTrue(all('--no-build' in c['args'] and '--pull' in c['args'] for c in cli if 'up' in c['args'] or 'run' in c['args']))
                    down = next(c['args'] for c in cli if 'down' in c['args'])
                    self.assertNotIn('--volumes', down)
                    selected = next(c['args'] for c in cli if 'original-selection' in c['args'])
                    self.assertEqual(selected[-6:], ['--tier','1','-k','original-selection','-m','not performance'])
                    self.assertEqual((root / 'probe').read_text(), 'invoked')
                    self.assertTrue((inputs / 'observations.json').exists())
                    images = json.loads((inputs / 'launch-images.json').read_text())['services']
                    profiles = json.loads((inputs / 'profiles.json').read_text())
                    self.assertEqual(set(images), {'init-agent', *module.compose.MANAGERS})
                    self.assertTrue(all(value['image'].startswith('sha256:') for value in images.values()))
                    for service in module.compose.MANAGERS:
                        self.assertEqual(images[service]['image'], profiles[service]['workload']['image']['reference'])
                finally:
                    if record.exists():
                        shutil.rmtree(Path(record.read_text()))
                    # Exact binds recorded by this inert run, never global cleanup.
                    for call in calls:
                        for value in call.get('args', []):
                            if value.endswith('/fixtures/compose.yaml') and str(ROOT / ('.scratch/runtime-service-' + OWNER)) in value:
                                retained = Path(value).parent.parent
                                if retained.exists():
                                    shutil.rmtree(retained)
                    if diagnostics.exists():
                        for created in set(diagnostics.glob(OWNER + '.*')) - before:
                            created.unlink()


if __name__ == '__main__':
    unittest.main()
