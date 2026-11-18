#!/usr/bin/env python3
"""Replay actual release assembly with public byte fixtures, without publishing.

This checks release graph/ZIP contents, not ELF ABI or real-image startup. Those
need the real multiarch build and owned packaged-container execution lanes.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tarfile
import tempfile
import unittest
import zipfile

import yaml

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = yaml.safe_load((ROOT / '.github/workflows/publish.yml').read_text())
SERVICES = ('api', 'executor', 'notifier', 'supervisor', 'worker', 'sensor')
AGENTS = ('attune-agent', 'attune-sensor-agent', 'attune-mcp', 'attune', 'attune-workload-runner')


def step(name):
    return next(item for item in WORKFLOW['jobs']['build-rust-bundles']['steps'] if item.get('name') == name)


class WorkloadReleasePackages(unittest.TestCase):
    def test_source_agent_zip_assembly_uses_only_the_once_built_platform_helper(self):
        source = (ROOT / 'docker/Dockerfile.agent').read_text()
        command = source.split('ARG SOURCE_DATE_EPOCH=0\nRUN ', 1)[1].split('\n\n', 1)[0]
        self.assertEqual(source.count('/build/workload-runner.zip /usr/local/share/attune/workload-runner.zip'), 2)
        self.assertIn('dist/attune-workload-runner.zip /usr/local/share/attune/workload-runner.zip',
                      (ROOT / 'docker/Dockerfile.agent-package').read_text())
        for arch, target in [('amd64', 'x86_64-unknown-linux-musl'), ('arm64', 'aarch64-unknown-linux-musl')]:
            with self.subTest(arch=arch), tempfile.TemporaryDirectory(prefix='attune-agent-zip-') as directory:
                root = Path(directory)
                (root / 'scripts').mkdir()
                shutil.copyfile(ROOT / 'scripts/package-cli-archive.py', root / 'scripts/package-cli-archive.py')
                tools = root / 'tools'
                tools.mkdir()
                resolve = tools / 'resolve-rust-target'
                resolve.write_text('#!/bin/sh\nprintf "%s\\n" "$RUST_TARGET"\n')
                resolve.chmod(0o755)
                helper = root / 'attune-workload-runner'
                helper.write_bytes(('owned-public-helper-' + arch).encode())
                (root / 'private-plan.json').write_text('owned-private-not-for-runner')
                subprocess.run(['sh', '-ec', command.replace('/build/', './')], cwd=root, check=True,
                               timeout=10, env={'PATH': str(tools) + os.pathsep + os.defpath,
                                                'RUST_TARGET': target, 'SOURCE_DATE_EPOCH': '1700000000'})
                output = root / 'workload-runner.zip'
                self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o444)
                with zipfile.ZipFile(output) as archive:
                    self.assertEqual(archive.namelist(), [f'linux-{arch}'])
                    self.assertEqual(archive.read(f'linux-{arch}'), helper.read_bytes())
                    self.assertEqual(stat.S_IMODE(archive.getinfo(f'linux-{arch}').external_attr >> 16), 0o755)
                self.assertNotIn(b'owned-private', output.read_bytes())

    def test_actual_build_commands_select_services_and_static_runner(self):
        with tempfile.TemporaryDirectory(prefix='attune-build-command-') as directory:
            root = Path(directory)
            capture = root / 'commands.jsonl'
            cargo = root / 'cargo'
            cargo.write_text('#!/usr/bin/env python3\nimport json,os,sys\nwith open(os.environ["CAPTURE"],"a") as output: output.write(json.dumps(sys.argv[1:])+"\\n")\n')
            cargo.chmod(0o755)
            for name in ('Build release binaries', 'Build static agent binaries'):
                command = step(name)['run'].replace('${{ matrix.service_target }}', 'service-fixture').replace('${{ matrix.musl_target }}', 'musl-fixture')
                subprocess.run(['bash', '-euo', 'pipefail', '-c', command], check=True, timeout=10,
                    env={'PATH': str(root) + os.pathsep + os.defpath, 'CAPTURE': str(capture)})
            service, agent = [json.loads(line) for line in capture.read_text().splitlines()]
            for command in (service, agent):
                self.assertEqual(command[:3], ['zigbuild', '--locked', '--release'])
            self.assertNotIn('attune-launch-controller', service)
            self.assertIn(['--bin', 'attune-workload-runner'], [agent[i:i+2] for i in range(len(agent)-1)])
            self.assertEqual(agent[agent.index('--target') + 1], 'musl-fixture')


    def test_provider_fixture_context_contains_only_exact_public_executables(self):
        dockerfile = (ROOT / 'docker/Dockerfile.workload-provider-test').read_text()
        copies = [line.split() for line in dockerfile.splitlines() if line.startswith('COPY ')]
        self.assertEqual([line[-2] for line in copies], ['attune-workload-runner', 'echo'])
        self.assertTrue(all(line[1] == '--chmod=0555' for line in copies))
        self.assertIn('BASE_IMAGE=python@sha256:', dockerfile)
        self.assertNotIn('privileged', dockerfile)

    def test_both_actual_assembly_scripts_produce_public_platform_zip_and_complete_bundle(self):
        for arch in ('amd64', 'arm64'):
            with self.subTest(arch=arch), tempfile.TemporaryDirectory(prefix='attune-release-graph-') as directory:
                root = Path(directory)
                (root / 'scripts').mkdir()
                shutil.copyfile(ROOT / 'scripts/package-cli-archive.py', root / 'scripts/package-cli-archive.py')
                fake_bin = root / 'tools'
                fake_bin.mkdir()
                # Only the controlled source-date query is needed by assembly.
                git = fake_bin / 'git'
                git.write_text('#!/bin/sh\n[ "$#" = 4 ] && [ "$1" = show ] && [ "$2" = -s ] && [ "$3" = --format=%ct ] || exit 1\nprintf "1700000000\\n"\n')
                git.chmod(0o755)
                service_dir = root / 'target/service-fixture/release'
                musl_dir = root / 'target/musl-fixture/release'
                service_dir.mkdir(parents=True)
                musl_dir.mkdir(parents=True)
                for name in SERVICES:
                    (service_dir / ('attune-' + name)).write_bytes(('public-' + name).encode())
                for name in AGENTS:
                    (musl_dir / name).write_bytes(('public-' + name + '-' + arch).encode())
                for crate, name in [('core-timer-sensor', 'attune-core-timer-sensor'), *[('core-actions', 'attune-core-' + action) for action in ('echo', 'sleep', 'noop', 'http_request')]]:
                    path = root / 'crates' / crate / 'target/musl-fixture/release' / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(b'public-pack-helper')
                scratch = root / 'scratch'
                scratch.mkdir()
                stale_stage = root / 'dist/runner' / arch
                stale_stage.mkdir(parents=True)
                (stale_stage / 'stale-private-plan.json').write_text('owned-private-not-for-runner')
                # Nothing near build outputs may accidentally enter Runner input.
                (musl_dir / 'private-plan.json').write_text('owned-private-not-for-runner')
                (root / 'controller-session.json').write_text('owned-private-not-for-runner')
                result = subprocess.run(['bash', '-euo', 'pipefail', '-c', step('Assemble binary bundle')['run']], cwd=root,
                    env={'PATH': str(fake_bin) + os.pathsep + os.defpath, 'TMPDIR': str(scratch), 'BUNDLE_ARCH': arch,
                         'SERVICE_RUST_TARGET': 'service-fixture', 'MUSL_TARGET': 'musl-fixture',
                         'COMMIT_SHA': 'a' * 40, 'GIT_REF': 'refs/tags/owned-fixture', 'IMAGE_TAG': 'owned-fixture', 'GITHUB_SHA': 'a' * 40},
                    capture_output=True, timeout=20)
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                self.assertEqual(list(scratch.iterdir()), [], 'owned Runner staging was not removed')
                output = root / f'dist/attune-workload-runner-linux-{arch}.zip'
                with zipfile.ZipFile(output) as archive:
                    self.assertEqual(archive.namelist(), [f'linux-{arch}'])
                    self.assertEqual(archive.read(f'linux-{arch}'), (musl_dir / 'attune-workload-runner').read_bytes())
                    self.assertEqual(stat.S_IMODE(archive.getinfo(f'linux-{arch}').external_attr >> 16), 0o755)
                self.assertEqual(output.with_suffix('.zip.sha256').read_text(), hashlib.sha256(output.read_bytes()).hexdigest() + '  ' + output.name + '\n')
                self.assertNotIn(b'owned-private', output.read_bytes())
                with tarfile.open(root / f'dist/attune-binaries-{arch}.tar.gz') as bundle:
                    self.assertNotIn('bin/attune-launch-controller', bundle.getnames())
                    self.assertIn('agent/attune-workload-runner', bundle.getnames())
                    self.assertEqual(bundle.extractfile('agent/attune-workload-runner.zip').read(), output.read_bytes())
                    self.assertNotIn('agent/private-plan.json', bundle.getnames())


if __name__ == '__main__':
    unittest.main()
