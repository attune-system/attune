#!/usr/bin/env python3
"""Explicit CI operator admission and once-built native inputs, never enrollment.

--plan is filesystem-only. --prepare has effects ONLY on the supplied Unix
endpoint; failures retain exact resources/observations for owner-led recovery.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import uuid
import zipfile

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location('native_compose', ROOT / 'scripts/prepare-native-compose.py')
compose = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(compose)
BUILD_MANAGERS = tuple(s for s in compose.MANAGERS if s != 'sensor-secondary')
LABEL = 'io.attune.test.owner'
NONCE = 'io.attune.test.native-input'


def write_json(path, value):
    with path.open('x', encoding='utf-8') as file:
        os.chmod(path, 0o600)
        json.dump(value, file, indent=2)
        file.write('\n')


def admission(path, owner):
    value = json.loads(path.read_text())
    if set(value) != {'engine_id', 'helper_pin_id', 'helper_object_id', 'object_version'}:
        raise ValueError('explicit CI operator namespace fields required')
    for key in ('engine_id', 'helper_pin_id', 'helper_object_id'):
        if type(value[key]) is not int or not 0 < value[key] <= 2**63 - 1:
            raise ValueError('positive explicit operator IDs required; never observed IDs')
    version = value['object_version']
    if not isinstance(version, str) or not 0 < len(version) <= 128 or not version.isascii():
        raise ValueError('bounded explicit ASCII candidate version required')
    if not re.fullmatch(r'[a-z0-9][a-z0-9_-]{0,47}', owner):
        raise ValueError('bounded explicit CI owner required')
    match = re.fullmatch(r'ci:([0-9a-f]{40}):([1-9][0-9]*):([1-9][0-9]*)', version)
    if not match or owner != f'ci-{match[2]}-{match[3]}':
        raise ValueError('explicit source/run/attempt version must match this CI owner')
    return value


def plan(root, project, owner, supplied):
    if not re.fullmatch(r'[a-z0-9][a-z0-9_-]{0,47}', project):
        raise ValueError('explicit safe owned project required')
    namespace = admission(supplied, owner)
    if (root.is_symlink() or not root.is_dir() or root.stat().st_mode & 0o077
            or root.stat().st_uid != os.getuid() or any(c in str(root) for c in ',\n\r')):
        raise ValueError('new private run input directory required')
    images = {service: f'{project}-native-{service}:candidate'
              for service in ('init-agent', *BUILD_MANAGERS)}
    images['sensor-secondary'] = images['sensor']
    write_json(root / 'admission.json', namespace)
    write_json(root / 'build-images.json', {'services': {s: {'image': image} for s, image in images.items()}})
    write_json(root / 'plan.json', {'owner': owner, 'project': project, 'images': images,
                                  'nonce': uuid.uuid4().hex})


class Docker:
    def __init__(self, socket):
        if (not socket.is_absolute() or socket.is_symlink()
                or not stat.S_ISSOCK(socket.stat().st_mode)):
            raise ValueError('explicit existing Unix endpoint required')
        self.host = 'unix://' + str(socket)

    def run(self, *args):
        # Never consult context/TLS or expose raw engine output/CP environment.
        result = subprocess.run(['docker', '--host', self.host, *args],
                                env={'PATH': os.environ['PATH']}, capture_output=True,
                                text=True, timeout=120, check=False)
        if result.returncode:
            raise ValueError('owned native input operation failed; evidence retained, no absence inferred')
        return result.stdout.strip()

    def json(self, *args):
        return json.loads(self.run(*args))


def runner_facts(path, platform):
    meta = path.lstat()
    if (not stat.S_ISREG(meta.st_mode) or meta.st_nlink != 1
            or meta.st_uid not in (0, 1000) or stat.S_IMODE(meta.st_mode) != 0o444
            or not 0 < meta.st_size <= 64 * 1024 * 1024):
        raise ValueError('Runner must satisfy manager1000 source ownership/link/mode/size policy')
    body = path.read_bytes()
    if len(body) != meta.st_size:
        raise ValueError('incomplete or changed public Runner capture')
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        if not 1 <= len(entries) <= 2 or len({e.filename for e in entries}) != len(entries):
            raise ValueError('bounded unique public Runner inventory required')
        expanded = 0
        for entry in entries:
            mode = entry.external_attr >> 16
            if (entry.filename not in ('linux-amd64', 'linux-arm64') or entry.is_dir()
                    or stat.S_IFMT(mode) not in (0, stat.S_IFREG) or not mode & 0o111
                    or entry.flag_bits & 1 or entry.file_size <= 0):
                raise ValueError('only regular executable public platform entries admitted')
            expanded += entry.file_size
            if expanded > 128 * 1024 * 1024:
                raise ValueError('public Runner expansion bound exceeded')
            actual = 0
            with archive.open(entry) as source:
                while chunk := source.read(65536):
                    actual += len(chunk)
                    if actual > entry.file_size:
                        raise ValueError('Runner entry expansion differs from inventory')
            if actual != entry.file_size:
                raise ValueError('incomplete Runner capture')
        archive.getinfo('linux-' + platform.split('/')[1])
    return dict(size_bytes=len(body), sha256=hashlib.sha256(body).hexdigest(),
                max_expanded_bytes=expanded, max_files=len(entries))


def one(observed):
    if not isinstance(observed, list) or len(observed) != 1:
        raise ValueError('one exact observed resource required')
    return observed[0]


def prepare(root, socket, docker=None):
    intent = json.loads((root / 'plan.json').read_text())
    owner, nonce = intent['owner'], intent['nonce']
    namespace = admission(root / 'admission.json', owner)
    docker = docker or Docker(socket)
    state = {'intent': intent, 'namespace': namespace, 'socket': str(socket), 'stage': 'observing'}
    receipt = root / 'observations.json'
    write_json(receipt, state)

    def save():
        # Fixed private file; no workload can access this directory or manifest.
        temporary = root / 'observations.next.json'
        write_json(temporary, state)
        temporary.replace(receipt)

    info = docker.json('info', '--format', '{{json .}}')
    arch = {'amd64': 'amd64', 'x86_64': 'amd64', 'arm64': 'arm64', 'aarch64': 'arm64'}.get(info.get('Architecture'))
    if info.get('OSType') != 'linux' or not arch or not info.get('ID'):
        raise ValueError('observed explicit Linux daemon identity/platform required')
    platform = 'linux/' + arch
    state['daemon'] = {'daemon_id': info['ID'], 'platform': platform}
    pins = {}
    for tag in set(intent['images'].values()):
        image = one(docker.json('image', 'inspect', tag))
        if (image.get('Os') != 'linux' or image.get('Architecture') != arch
                or not re.fullmatch(r'sha256:[0-9a-f]{64}', image.get('Id', ''))):
            raise ValueError('once-built image platform/immutable ID required')
        pins[tag] = {'reference': image['Id'], 'sha256': image['Id'][7:]}
    state['images'] = pins
    # Randomness scopes fresh resource names; it never becomes admission identity.
    prefix = 'attune-native-' + owner + '-' + nonce[:12]
    network = prefix + '-bridge'
    volumes = {s: {kind: prefix + '-' + s + '-' + kind for kind in ('cache', 'custody')}
               for s in compose.MANAGERS}
    state.update(network=network, volumes=volumes, stage='creating-inputs')
    save()
    labels = ['--label', LABEL + '=' + owner, '--label', NONCE + '=' + nonce]
    # Successful listings are only collision checks, never stopped-writer proof.
    existing_volumes = set(docker.run('volume', 'ls', '--format', '{{.Name}}').splitlines())
    existing_networks = set(docker.run('network', 'ls', '--format', '{{.Name}}').splitlines())
    if network in existing_networks or any(v in existing_volumes for vs in volumes.values() for v in vs.values()):
        raise ValueError('refusing pre-existing native input names')
    network_id = docker.run('network', 'create', '--driver', 'bridge', *labels, network)
    if not re.fullmatch('[0-9a-f]{64}', network_id):
        raise ValueError('unknown network creation; reconcile retained intent')
    state['network_id'] = network_id
    save()
    bridge = one(docker.json('network', 'inspect', network))
    if (bridge.get('Id') != network_id or bridge.get('Name') != network
            or bridge.get('Driver') != 'bridge' or bridge.get('Containers') != {}
            or (bridge.get('Labels') or {}).get(LABEL) != owner
            or (bridge.get('Labels') or {}).get(NONCE) != nonce):
        raise ValueError('exact fresh ordinary owned bridge required')
    for vs in volumes.values():
        for volume in vs.values():
            if docker.run('volume', 'create', '--driver', 'local', *labels, volume) != volume:
                raise ValueError('unknown volume create acknowledgement')
            observed = one(docker.json('volume', 'inspect', volume))
            if (observed.get('Name') != volume or observed.get('Driver') != 'local'
                    or 'Options' not in observed or observed['Options'] not in (None, {})
                    or (observed.get('Labels') or {}).get(NONCE) != nonce
                    or (observed.get('Labels') or {}).get(LABEL) != owner):
                raise ValueError('volume creation ownership/driver/options differ')
            if docker.run('ps', '--all', '--filter', 'volume=' + volume, '--format', '{{json .}}'):
                raise ValueError('fresh volume has container users')
    # Fixed initializer sees only fresh private inputs; final preflight repeats
    # resource binding after its exact stopped/removal acknowledgement.
    public = root / 'runner.zip'
    helper_image = pins[intent['images']['init-agent']]
    mounts = ['--mount', f'type=bind,src={root},dst=/ci-input']
    checks, initialize = [], []
    for service, vs in volumes.items():
        for kind, volume in vs.items():
            target = '/native/' + service + '-' + kind
            mounts += ['--mount', f'type=volume,src={volume},dst={target}']
            checks.append(f'entries=$(ls -Aq {target}) && test -z "$entries"')
            initialize += [f'chown 1000:1000 {target}', f'chmod 0700 {target}']
    fixed = ' && '.join([*checks, *initialize, 'test ! -e /ci-input/runner.zip',
                         'test ! -L /ci-input/runner.zip',
                         'cp /usr/local/share/attune/workload-runner.zip /ci-input/runner.zip',
                         'chown 0:0 /ci-input/runner.zip', 'chmod 0444 /ci-input/runner.zip'])
    state['stage'] = 'initializer-create-unknown'
    save()
    container = docker.run('create', '--name', prefix + '-initializer', *labels,
                          '--network', 'none', '--read-only', '--pull', 'never', '--user', '0:0',
                          '--memory', '536870912', '--pids-limit', '128',
                          '--cap-drop', 'ALL', '--cap-add', 'CHOWN', '--cap-add', 'FOWNER',
                          '--cap-add', 'DAC_OVERRIDE', '--security-opt', 'no-new-privileges',
                          *mounts, '--entrypoint', '/bin/sh', helper_image['reference'], '-ec', fixed)
    if not re.fullmatch('[0-9a-f]{64}', container):
        raise ValueError('unknown initializer create; retain exact intent, never remove by name')
    state.update(initializer_id=container, stage='initializer-retained')
    save()
    retained = one(docker.json('inspect', container))
    expected_mounts = {str(root): '/ci-input', **{v: '/native/' + s + '-' + k for s, vs in volumes.items() for k, v in vs.items()}}
    actual_mounts = {(m.get('Name') if m.get('Type') == 'volume' else m.get('Source')): m.get('Destination')
                     for m in retained.get('Mounts', [])}
    if (retained.get('Id') != container or retained.get('Image') != helper_image['reference']
            or (retained.get('Config', {}).get('Labels') or {}).get(NONCE) != nonce
            or (retained.get('Config', {}).get('Labels') or {}).get(LABEL) != owner
            or retained.get('Config', {}).get('Entrypoint') != ['/bin/sh']
            or retained.get('Config', {}).get('Cmd') != ['-ec', fixed]
            or retained.get('HostConfig', {}).get('NetworkMode') != 'none'
            or retained.get('HostConfig', {}).get('ReadonlyRootfs') is not True
            or retained.get('HostConfig', {}).get('Privileged') is not False
            or retained.get('HostConfig', {}).get('Memory') != 536870912
            or retained.get('HostConfig', {}).get('PidsLimit') != 128
            or retained.get('State', {}).get('Status') != 'created'
            or retained.get('State', {}).get('Running') is not False
            or len(retained.get('Mounts', [])) != len(expected_mounts)
            or actual_mounts != expected_mounts):
        raise ValueError('initializer exact image/ownership/mount evidence differs')
    docker.run('start', container)
    if docker.run('wait', container) != '0':
        raise ValueError('initializer exit not positively successful')
    stopped = one(docker.json('inspect', container))
    if (stopped.get('Id') != container or stopped.get('State', {}).get('Running') is not False
            or stopped['State'].get('Status') != 'exited' or stopped['State'].get('ExitCode') != 0
            or stopped['State'].get('OOMKilled') is not False or stopped['State'].get('Error') != ''):
        raise ValueError('exact stopped initializer required')
    facts = runner_facts(public, platform)
    state.update(runner=facts, stage='initializer-stopped')
    save()
    if docker.run('rm', container) != container:
        raise ValueError('exact stopped initializer removal not acknowledged')
    state.update(stage='initializer-removed', initializer_removed=True)
    save()
    profiles = {}
    # Same budgets as the existing owning Docker worker fixture, never raised.
    for service in compose.MANAGERS:
        workload = dict(image=pins[intent['images'][service]], platform=platform,
                        cache_root='/var/lib/attune/workloads', helper_archive='/opt/attune/public/workload-runner.zip',
                        memory_bytes=536870912, scratch_bytes=134217728,
                        storage=dict(compressed_bytes=67108864, expanded_bytes=134217728, entries=4096),
                        helper=dict(pin_id=namespace['helper_pin_id'], kind='runner', object_id=namespace['helper_object_id'],
                                    object_version=namespace['object_version'], **facts),
                        backend=dict(kind='docker', socket='/var/run/docker.sock', cache_volume=volumes[service]['cache'],
                                     network=network, binding=dict(engine_id=namespace['engine_id'], **state['daemon']),
                                     pids_limit=128, helper_image=helper_image))
        profiles[service] = dict(workload=workload, runner_source=str(public), custody_volume=volumes[service]['custody'])
    write_json(root / 'profiles.json', profiles)
    write_json(root / 'launch-images.json', {'services': {
        'init-agent': {'image': helper_image['reference']},
        **{s: {'image': profiles[s]['workload']['image']['reference']} for s in compose.MANAGERS},
    }})
    state['stage'] = 'profiles-retained'
    save()
    return profiles


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--admission', type=Path)
    parser.add_argument('--owner')
    parser.add_argument('--project')
    parser.add_argument('--socket', type=Path)
    parser.add_argument('--plan', action='store_true')
    args = parser.parse_args()
    if args.root.is_symlink():
        parser.error('run input root cannot be an alias')
    root = args.root.resolve(strict=True)
    if args.plan:
        plan(root, args.project, args.owner, args.admission)
    else:
        if args.socket is None:
            parser.error('--socket required for actual CI input preparation')
        prepare(root, args.socket)


if __name__ == '__main__':
    main()
