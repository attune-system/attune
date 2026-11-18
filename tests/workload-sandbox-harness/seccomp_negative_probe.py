"""Owned amd64 container probe; negative controls are not deployment evidence."""
import ctypes
import errno
import json
import os
import platform
import sys

if platform.machine() != 'x86_64':
    raise SystemExit('This diagnostic profile is verified only on amd64')
libc = ctypes.CDLL(None, use_errno=True)
uid, gid = os.getuid(), os.getgid()
flags = 0x3c020011
pid = libc.syscall(56, flags, 0, 0, 0, 0)
if pid < 0:
    raise OSError(ctypes.get_errno(), 'exact Bubblewrap namespace clone denied')
if pid:
    _, status = os.waitpid(pid, 0)
    sys.exit(os.waitstatus_to_exitcode(status))
try:
    for name, value in [('setgroups', 'deny'), ('uid_map', f'0 {uid} 1'), ('gid_map', f'0 {gid} 1')]:
        with open(f'/proc/self/{name}', 'w') as stream:
            stream.write(value)
    results = {}
    for label, number, arguments in [
        ('network_namespace', 272, [0x40000000]),
        ('other_unshare', 272, [0x04000000]),
        ('arbitrary_setns', 308, [-1, 0x10000000]),
        ('unobserved_mount_flags', 165, [b'none', b'/definitely-absent', b'tmpfs', 0x40000000, 0]),
    ]:
        ctypes.set_errno(0)
        result = libc.syscall(number, *arguments)
        results[label] = result == -1 and ctypes.get_errno() == errno.EPERM
    print(json.dumps({'negative_controls': results, 'deployment_support_established': False}), flush=True)
    os._exit(0 if all(results.values()) else 1)
except BaseException as error:
    print(repr(error), flush=True)
    os._exit(1)
