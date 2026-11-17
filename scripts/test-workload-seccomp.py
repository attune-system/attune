#!/usr/bin/env python3
"""Offline security invariants for the experimental, non-deployable profile."""
import hashlib
import json
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent.parent
baseline_bytes = (root / 'docker/seccomp/moby-f8624556-default.json').read_bytes()
assert hashlib.sha256(baseline_bytes).hexdigest() == '3320527d4568265ac97a077609b1bfd4cfd0d789eeb9239b7e094b1526675406'
base = json.loads(baseline_bytes)
profile_bytes = (root / 'docker/seccomp/workload-bwrap-amd64.json').read_bytes()
expected = subprocess.check_output(['python3', str(root / 'scripts/generate-workload-seccomp.py')])
assert profile_bytes == expected
profile = json.loads(profile_bytes)
assert profile['defaultAction'] == 'SCMP_ACT_ERRNO'
assert profile['defaultErrnoRet'] == 1
assert profile['syscalls'][:len(base['syscalls'])] == base['syscalls']
extra = profile['syscalls'][len(base['syscalls']):]
assert {name for rule in extra for name in rule['names']} == {'clone', 'unshare', 'mount', 'pivot_root', 'umount2'}
assert all(rule['includes'] == {'arches': ['amd64']} for rule in extra)
clone = next(rule for rule in extra if rule['names'] == ['clone'])
assert clone['args'] == [{'index': 0, 'value': 0x3c020011, 'op': 'SCMP_CMP_EQ'}]
assert not clone['args'][0]['value'] & (0x40000000 | 0x02000000)  # NET / CGROUP
unshare = next(rule for rule in extra if rule['names'] == ['unshare'])
assert unshare['args'] == [{'index': 0, 'value': 0x10000000, 'op': 'SCMP_CMP_EQ'}]
print('Pinned default deny and namespace/mount allowance invariants verified; no deployment support implied.')
