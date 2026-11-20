#!/usr/bin/env python3
"""Offline DBA grants only. Requires psycopg 3; creates/adopts no roles or secrets."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import stat
import sys


def private_json(path, limit):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    try:
        before = os.fstat(fd)
        if not (stat.S_ISREG(before.st_mode) and stat.S_IMODE(before.st_mode) == 0o600
                and before.st_uid == os.geteuid() and before.st_nlink == 1 and before.st_size <= limit):
            raise ValueError('private input')
        with os.fdopen(fd, "rb", closefd=False) as stream:
            data = stream.read(limit + 1)
        after = os.fstat(fd)
        named = os.stat(path, follow_symlinks=False)
        if len(data) > limit or any(getattr(before, k) != getattr(after, k) for k in ("st_dev","st_ino","st_mode","st_uid","st_nlink","st_size","st_mtime_ns","st_ctime_ns")) or named.st_ino != before.st_ino or named.st_dev != before.st_dev:
            raise ValueError('changed input')
        return json.loads(data)
    finally:
        os.close(fd)


class RedactedParser(argparse.ArgumentParser):
    def error(self, message):
        raise ValueError("invalid arguments")


def main():
    if sys.platform != 'linux' or ctypes.CDLL(None).prctl(4, 0, 0, 0, 0) != 0:
        raise ValueError('parent protection')
    os.umask(0o077)
    os.environ.clear()
    parser = RedactedParser(description=__doc__)
    parser.add_argument('--dba-credentials', required=True)
    parser.add_argument('--inventory', required=True)
    args = parser.parse_args()
    credential = private_json(args.dba_credentials, 16384)
    inventory = private_json(args.inventory, 65536)
    if set(credential) != {'database_url'}:
        raise ValueError('credential shape')
    from urllib.parse import urlsplit, parse_qs
    url = urlsplit(credential['database_url'])
    if (url.scheme not in ('postgres','postgresql') or not url.username or not url.password
            or not url.hostname or not url.path.strip('/') or url.fragment
            or set(parse_qs(url.query)) - {'sslmode'}):
        raise ValueError('explicit connection required')
    import psycopg
    with psycopg.connect(credential['database_url'], connect_timeout=10,
                          application_name='attune-offline-dba', options='-c search_path=pg_catalog,pg_temp -c statement_timeout=15000 -c lock_timeout=10000 -c idle_in_transaction_session_timeout=20000') as conn:
        with conn.cursor() as cur:
            cur.execute('SELECT current_user=session_user AND rolsuper FROM pg_catalog.pg_roles WHERE rolname=session_user')
            if cur.fetchone() != (True,):
                raise ValueError('explicit DBA session required')
            cur.execute('CREATE TEMP TABLE rollout_provision_input(document JSONB) ON COMMIT DROP')
            cur.execute('INSERT INTO rollout_provision_input VALUES (%s::jsonb)', (json.dumps(inventory),))
            root = Path(__file__).resolve().parents[1]
            cur.execute((root / 'deployment/rollout-admin/provision.sql').read_text())
            from hashlib import sha384
            from psycopg import sql
            cur.execute(sql.SQL('SELECT version,checksum,success FROM {}._sqlx_migrations').format(sql.Identifier(inventory['schema'])))
            actual = {(v, bytes(checksum), success) for v, checksum, success in cur.fetchall()}
            expected = {(int(p.name.split('_')[0]), sha384(p.read_bytes()).digest(), True) for p in (root / 'migrations').glob('*.sql')}
            if actual != expected:
                raise ValueError('frozen SQLx ledger differs')
    print('{"configured":true}')


if __name__ == '__main__':
    try:
        main()
    except Exception:
        print('{"error":"provisioning_denied_or_commit_unknown"}', file=sys.stderr)
        sys.exit(1)
