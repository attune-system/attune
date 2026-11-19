#!/usr/bin/env python3
"""Prepare explicit native Docker acceptance configuration; static by default.

Optional --verify-owned performs read-only checks at the explicitly supplied
Unix socket. This does not enable production isolation. All fixed profiles, including the
optional toolchain worker and both sensors, require operator-supplied current
WorkloadConfig and real public Runner bytes. Volumes/network must already exist
on the explicitly bound daemon; the adapter positively verifies their binding.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import re
import stat
import subprocess
import zipfile
import yaml

ROOT = Path(__file__).resolve().parent.parent
MANAGERS = ('worker-shell', 'worker-python', 'worker-node', 'worker-full',
            'worker-toolchains', 'sensor', 'sensor-secondary')


def prepare(profiles, base, output, socket_gid, socket_source="/var/run/docker.sock", validate_only=False):
    if not Path(socket_source).is_absolute() or any(c in str(socket_source) for c in ":$\n\r"):
        raise ValueError("explicit absolute Unix socket path required")
    if set(profiles) != set(MANAGERS):
        raise ValueError('supply every configured manager, including toolchains and both sensors')
    if socket_gid < 0:
        raise ValueError('explicit socket group ID required')
    compose = yaml.safe_load((ROOT / 'docker-compose.yaml').read_text())
    source_services = copy.deepcopy(compose['services'])
    configs = {}
    seen = set()
    native_networks = {}
    for service in MANAGERS:
        profile = profiles[service]
        if set(profile) != {'workload', 'runner_source', 'custody_volume'}:
            raise ValueError('profile requires workload, runner_source and private custody_volume')
        workload = copy.deepcopy(profile['workload'])
        if set(workload) != {'image', 'platform', 'cache_root', 'memory_bytes', 'scratch_bytes', 'storage', 'helper', 'helper_archive', 'backend'}:
            raise ValueError('use the current fixed WorkloadConfig fields')
        backend = workload['backend']
        if set(backend) != {'kind', 'socket', 'cache_volume', 'network', 'binding', 'pids_limit', 'helper_image'} or backend['kind'] != 'docker':
            raise ValueError('explicit closed Docker backend required')
        if backend['socket'] != '/var/run/docker.sock' or workload['cache_root'] != '/var/lib/attune/workloads':
            raise ValueError('packaging requires fixed manager socket/cache paths')
        if workload['helper_archive'] != '/opt/attune/public/workload-runner.zip':
            raise ValueError('packaging requires the fixed public Runner archive path')
        if workload['platform'] not in ('linux/amd64','linux/arm64') or backend['binding']['platform'] != workload['platform']:
            raise ValueError('exact supported daemon platform required')
        if not backend['binding']['daemon_id'] or backend['binding']['engine_id'] <= 0:
            raise ValueError('expected daemon identity and operator-supplied native binding ID required')
        for image in (workload['image'], backend['helper_image']):
            digest = image['sha256']
            if not re.fullmatch('[0-9a-f]{64}', digest) or not (image['reference'] == 'sha256:' + digest or image['reference'].endswith('@sha256:' + digest)):
                raise ValueError('immutable actual image pin required')
        if backend['network'] in ('host','none') or not re.fullmatch('[A-Za-z0-9][A-Za-z0-9_.-]*', backend['network']):
            raise ValueError('explicit ordinary network required')
        for volume in (backend['cache_volume'], profile['custody_volume']):
            if not re.fullmatch('[A-Za-z0-9][A-Za-z0-9_.-]*', volume) or volume in seen:
                raise ValueError('every cache and custody volume must be private to one manager')
            seen.add(volume)
            compose['volumes'][volume] = {'external': True, 'name': volume}
        supplied_source = Path(profile['runner_source'])
        if supplied_source.is_symlink():
            raise ValueError('Runner source cannot be an alias')
        source = supplied_source.resolve(strict=True)
        pin = workload['helper']
        if pin['kind'] != 'runner' or pin['pin_id'] <= 0 or pin['object_id'] <= 0 or not pin['object_version']:
            raise ValueError('exact admitted Runner object pin required')
        if source.is_symlink() or not source.is_file() or source.stat().st_mode & 0o022:
            raise ValueError('public Runner must be a regular non-writable source')
        body = source.read_bytes()
        if len(body) != pin['size_bytes'] or hashlib.sha256(body).hexdigest() != pin['sha256']:
            raise ValueError('public Runner source does not match admitted pin')
        with zipfile.ZipFile(source) as archive:
            selected = archive.getinfo('linux-' + workload['platform'].split('/')[1])
            if not selected.external_attr >> 16 & 0o111 or selected.is_dir():
                raise ValueError('Runner archive lacks executable platform entry')
        config = copy.deepcopy(base)
        config['workload'] = workload
        config['runtime_envs_dir'] = '/var/lib/attune/custody/runtime-envs'
        config['packs_base_dir'] = '/var/lib/attune/custody/packs'
        config['artifacts_dir'] = '/var/lib/attune/custody/artifacts'
        config.setdefault('packs', {})['transport'] = 'api'
        config.setdefault('worker', {}).setdefault('log_delivery', {})['spool_dir'] = '/var/lib/attune/custody/spool'
        config.setdefault('artifacts', {})['transport'] = 'api'
        configs[service] = config
        original = copy.deepcopy(source_services[service])
        compose['services'][service] = original
        # Materialize extends so optional toolchains receive their own private inputs.
        if 'extends' in original:
            merged = copy.deepcopy(source_services[original['extends']['service']])
            merged['environment'].update(original.get('environment', {}))
            merged.update({k:v for k,v in original.items() if k not in ('extends','environment')})
            compose['services'][service] = original = merged
        original['user'] = '1000:1000'
        original['group_add'] = [str(socket_gid)]
        original['entrypoint'] = ['/opt/attune/agent/' + ('attune-sensor-agent' if service.startswith('sensor') else 'attune-agent')]
        original['environment']['HOME'] = '/tmp'
        original['environment']['ATTUNE_CONFIG'] = '/opt/attune/config/native.yaml'
        # Sensor's explicit legacy override otherwise defeats the private config.
        original['environment']['ATTUNE_PACKS_BASE_DIR'] = config['packs_base_dir']
        # Existing manager CP credentials, labels, recipe admission and profiles stay intact.
        original['volumes'] = [v for v in original['volumes'] if v.split(':')[1] not in ('/opt/attune/runtime_envs','/opt/attune/artifacts','/opt/attune/packs.dev','/var/lib/attune/workloads','/var/lib/attune/custody')]
        original['volumes'] = [v.replace('packs_data:/opt/attune/packs:rw', 'packs_data:/opt/attune/packs:ro') for v in original['volumes']]
        original['volumes'] += [f'{output / (service + ".yaml")}:/opt/attune/config/native.yaml:ro',
            f'{source}:/opt/attune/public/workload-runner.zip:ro',
            f'{backend["cache_volume"]}:/var/lib/attune/workloads',
            f'{profile["custody_volume"]}:/var/lib/attune/custody',
            f'{socket_source}:/var/run/docker.sock']
        original.setdefault('networks', [])
        key = native_networks.get(backend['network'])
        if key is None:
            key = 'native-' + service
            native_networks[backend['network']] = key
            compose.setdefault('networks', {})[key] = {'external': True, 'name': backend['network']}
            # Join each actual bridge once, even when profiles share it.
            # Authentication still gates guest API access. DB/MQ never join.
            compose['services']['api']['networks'].append(key)
        if isinstance(original['networks'], list):
            original['networks'] = list(original['networks']) + [key]
        else:
            raise ValueError('unexpected manager network representation')
    # Write only after all configured sources pass static checks. Runtime config
    # validation and binding probes remain authoritative; static shape is no proof.
    if validate_only:
        return
    output.mkdir(mode=0o700)
    for service, config in configs.items():
        (output / (service + '.yaml')).write_text(yaml.safe_dump(config, sort_keys=False))
    (output / 'compose.yaml').write_text(yaml.safe_dump(compose, sort_keys=False))


def verify_owned(profiles, socket_source, owner, docker_json=None):
    """Read-only preflight. Observation checks supplied authority; it never mints it.

    The externally prepared inputs remain retained after Compose teardown. This
    helper cannot establish native per-attempt stop/removal receipts.
    """
    if not re.fullmatch('[a-z0-9][a-z0-9_-]{0,47}', owner):
        raise ValueError('explicit safe run owner required')
    if set(profiles) != set(MANAGERS):
        raise ValueError('all seven native profiles required')
    socket = Path(socket_source)
    if not socket.is_absolute() or not stat.S_ISSOCK(socket.stat().st_mode):
        raise ValueError('explicit existing Unix socket required')
    if docker_json is None:
        def docker_json(*args):
            result = subprocess.run(['docker', '--host', 'unix://' + str(socket), *args],
                                    capture_output=True, text=True, timeout=30, check=False)
            if result.returncode:
                raise ValueError('explicit endpoint observation failed; no absence inferred')
            if args[0] == 'ps':
                return [json.loads(line) for line in result.stdout.splitlines()]
            return json.loads(result.stdout)
    info = docker_json('info', '--format', '{{json .}}')
    arch = {'amd64': 'amd64', 'x86_64': 'amd64', 'arm64': 'arm64', 'aarch64': 'arm64'}.get(info.get('Architecture'))
    if info.get('OSType') != 'linux' or arch is None:
        raise ValueError('unsupported observed daemon platform')
    platform = 'linux/' + arch
    checked_volumes, checked_networks, checked_images = set(), set(), set()
    for profile in profiles.values():
        workload = profile['workload']
        backend = workload['backend']
        binding = backend['binding']
        if (binding['daemon_id'] != info.get('ID') or binding['platform'] != platform
                or workload['platform'] != platform):
            raise ValueError('supplied daemon/platform binding differs from explicit endpoint')
        for volume in (backend['cache_volume'], profile['custody_volume']):
            if volume in checked_volumes:
                raise ValueError('shared native writable volume')
            checked_volumes.add(volume)
            observed = docker_json('volume', 'inspect', volume)
            if (len(observed) != 1 or observed[0].get('Name') != volume
                    or (observed[0].get('Labels') or {}).get('io.attune.test.owner') != owner
                    or observed[0].get('Driver') != 'local' or 'Options' not in observed[0]
                    or observed[0]['Options'] not in (None, {})):
                raise ValueError('native volume must be an exact run-owned ordinary local volume')
            users = docker_json('ps', '--all', '--filter', 'volume=' + volume, '--format', '{{json .}}')
            if users:
                raise ValueError('native volume already has container users')
        network = backend['network']
        if network not in checked_networks:
            checked_networks.add(network)
            observed = docker_json('network', 'inspect', network)
            if (len(observed) != 1 or observed[0].get('Name') != network
                    or (observed[0].get('Labels') or {}).get('io.attune.test.owner') != owner
                    or observed[0].get('Driver') != 'bridge' or observed[0].get('Containers') != {}):
                raise ValueError('native network must be an unused exact run-owned bridge')
        for image in (workload['image'], backend['helper_image']):
            reference = image['reference']
            if reference in checked_images:
                continue
            checked_images.add(reference)
            observed = docker_json('image', 'inspect', reference)
            if (len(observed) != 1 or observed[0].get('Os') != 'linux'
                    or observed[0].get('Architecture') != arch
                    or not (observed[0].get('Id') == reference
                            or reference in (observed[0].get('RepoDigests') or []))):
                raise ValueError('actual prebuilt image/platform does not match supplied pin')
    return socket.stat().st_gid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profiles', required=True, type=Path, help='JSON map of every manager to actual operator inputs')
    parser.add_argument('--config', default=ROOT / 'docker/distributable/config.docker.yaml', type=Path)
    parser.add_argument('--output', required=True, type=Path, help='new directory; never overwritten')
    parser.add_argument('--socket-gid', type=int)
    parser.add_argument('--socket-source', default='/var/run/docker.sock', type=Path)
    parser.add_argument('--verify-owned', metavar='RUN_ID', help='read-only explicit-daemon/owned-input checks; derive socket GID')
    args = parser.parse_args()
    profiles = json.loads(args.profiles.read_text())
    gid = args.socket_gid
    base = yaml.safe_load(args.config.read_text())
    if args.verify_owned:
        prepare(profiles, base, args.output.resolve(), 0, args.socket_source, validate_only=True)
        gid = verify_owned(profiles, args.socket_source, args.verify_owned)
    if gid is None:
        parser.error('--socket-gid or --verify-owned is required')
    prepare(profiles, base, args.output.resolve(), gid, args.socket_source)


if __name__ == '__main__':
    main()
