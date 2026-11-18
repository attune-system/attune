#!/usr/bin/env python3
"""Derive the diagnostic Bubblewrap profile from a byte-pinned Moby baseline.

This profile alone does not make Docker's locked/masked procfs compatible.
Do not deploy it as proof of isolated-workload support.
"""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
profile = json.loads((ROOT / 'docker/seccomp/moby-f8624556-default.json').read_text())
assert profile['defaultAction'] == 'SCMP_ACT_ERRNO'

def allow(names, args=None):
    rule = {'names': names, 'action': 'SCMP_ACT_ALLOW', 'includes': {'arches': ['amd64']}}
    if args is not None:
        rule['args'] = args
    profile['syscalls'].append(rule)

# Observed on packaged Debian 0.8.0-2+deb12u1, nonroot with all outer
# capabilities dropped. No NET/CGROUP namespace or arbitrary setns permission.
allow(['clone'], [{'index': 0, 'value': 0x3c020011, 'op': 'SCMP_CMP_EQ'}])
allow(['unshare'], [{'index': 0, 'value': 0x10000000, 'op': 'SCMP_CMP_EQ'}])
allow(['pivot_root'])  # pointer arguments cannot be meaningfully filtered
allow(['umount2'], [{'index': 1, 'value': 2, 'op': 'SCMP_CMP_EQ'}])
for flags in [0x8c000, 6, 0xc0edd000, 0xd000, 14, 10, 0x4c000]:
    allow(['mount'], [{'index': 3, 'value': flags, 'op': 'SCMP_CMP_EQ'}])
# Bind-remount security flags; only read-only/atime variants may differ.
atime_and_ro = 1 | 8 | 1024 | 2048 | (1 << 21) | (1 << 24) | (1 << 25)
allow(['mount'], [{'index': 3, 'value': ((1 << 64) - 1) ^ atime_and_ro,
                    'valueTwo': 2 | 4 | 32 | 4096 | 32768, 'op': 'SCMP_CMP_MASKED_EQ'}])
print(json.dumps(profile, indent=2))
