#!/usr/bin/python3
"""Atomically exchange two pathnames with Linux renameat2(2)."""

import ctypes
import errno
import os
import sys


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} PATH_A PATH_B", file=sys.stderr)
        return 2

    libc = ctypes.CDLL(None, use_errno=True)
    try:
        renameat2 = libc.renameat2
    except AttributeError:
        print("renameat2 is unavailable in the installed C library", file=sys.stderr)
        return 1

    renameat2.argtypes = [
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_uint,
    ]
    renameat2.restype = ctypes.c_int
    at_fdcwd = -100
    rename_exchange = 2
    first = os.fsencode(sys.argv[1])
    second = os.fsencode(sys.argv[2])

    if renameat2(at_fdcwd, first, at_fdcwd, second, rename_exchange) == 0:
        return 0

    error = ctypes.get_errno()
    if error in (errno.ENOSYS, errno.EINVAL, errno.EOPNOTSUPP):
        print(
            "atomic pathname exchange is unsupported by this kernel or filesystem",
            file=sys.stderr,
        )
    else:
        print(os.strerror(error), file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
