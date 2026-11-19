#!/usr/bin/env python3
"""Prepare an explicit native Docker acceptance configuration without contacting Docker.

This does not enable production isolation. All fixed profiles, including the
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
import zipfile
import yaml

ROOT = Path(__file__).resolve().parent.parent
MANAGERS = ('worker-shell', 'worker-python', 'worker-node', 'worker-full',
            'worker-toolchains', 'sensor', 'sensor-secondary')


def prepare(profiles, base, output, socket_gid):
    if set(profiles) != set(MANAGERS):
        raise ValueError('supply every configured manager, including toolchains and both sensors')
    if socket_gid < 0:
        raise ValueError('explicit socket group ID required')
    compose = yaml.safe_load((ROOT / 'docker-compose.yaml').read_text())
    source_services = copy.deepcopy(compose['services'])
    configs = {}
    seen = set()
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
            raise ValueError('observed daemon identity and admitted engine ID required')
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
        # Existing manager CP credentials, labels, recipe admission and profiles stay intact.
        original['volumes'] = [v for v in original['volumes'] if v.split(':')[1] not in ('/opt/attune/runtime_envs','/opt/attune/artifacts','/opt/attune/packs.dev','/var/lib/attune/workloads','/var/lib/attune/custody')]
        original['volumes'] = [v.replace('packs_data:/opt/attune/packs:rw', 'packs_data:/opt/attune/packs:ro') for v in original['volumes']]
        original['volumes'] += [f'{output / (service + ".yaml")}:/opt/attune/config/native.yaml:ro',
            f'{source}:/opt/attune/public/workload-runner.zip:ro',
            f'{backend["cache_volume"]}:/var/lib/attune/workloads',
            f'{profile["custody_volume"]}:/var/lib/attune/custody',
            '/var/run/docker.sock:/var/run/docker.sock']
        original.setdefault('networks', [])
        key = 'native-' + service
        compose.setdefault('networks', {})[key] = {'external': True, 'name': backend['network']}
        if isinstance(original['networks'], list):
            original['networks'] = list(original['networks']) + [key]
        else:
            raise ValueError('unexpected manager network representation')
    # Write only after all configured sources pass static checks. Runtime config
    # validation and binding probes remain authoritative; static shape is no proof.
    output.mkdir(mode=0o700)
    for service, config in configs.items():
        (output / (service + '.yaml')).write_text(yaml.safe_dump(config, sort_keys=False))
    (output / 'compose.yaml').write_text(yaml.safe_dump(compose, sort_keys=False))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profiles', required=True, type=Path, help='JSON map of every manager to actual operator inputs')
    parser.add_argument('--config', default=ROOT / 'docker/distributable/config.docker.yaml', type=Path)
    parser.add_argument('--output', required=True, type=Path, help='new directory; never overwritten')
    parser.add_argument('--socket-gid', required=True, type=int)
    args = parser.parse_args()
    prepare(json.loads(args.profiles.read_text()), yaml.safe_load(args.config.read_text()), args.output.resolve(), args.socket_gid)


if __name__ == '__main__':
    main()
