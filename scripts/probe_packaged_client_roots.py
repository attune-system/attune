#!/usr/bin/env python3
"""Initialize real packaged CLI/MCP clients, not offline build-info shortcuts.

The caller owns the fresh Compose project, its already-built init-agent image
and healthy API. No credentials or private child output enter diagnostics.
"""
import json
import subprocess
import sys

API = 'http://api:8080'
MISSING_ROOTS = b'No CA certificates were loaded from the system'


def command(compose, binary, empty_roots):
    args = [*compose, 'run', '--rm', '--no-deps', '-T',
            '--entrypoint', '/usr/local/bin/' + binary, '-e', 'HOME=/tmp']
    if empty_roots:
        args += ['-e', 'SSL_CERT_FILE=/dev/null',
                 '-e', 'SSL_CERT_DIR=/attune-owned-probe-no-certificates']
    args += ['init-agent', '--api-url', API]
    args += ['--output', 'json', 'info'] if binary == 'attune' else ['--info']
    return args


def check(result, binary, empty_roots):
    # These fixed --info paths return compiled public build metadata only.
    if len(result.stdout) + len(result.stderr) > 65536:
        return False
    if empty_roots:
        return result.returncode != 0 and MISSING_ROOTS in result.stderr
    if result.returncode != 0:
        return False
    try:
        report = json.loads(result.stdout)
        return (report['local']['binary'] == binary
                and report['server']['status'] == 'connected'
                and report['local']['version'] == report['server']['version'])
    except (ValueError, TypeError, KeyError):
        return False


def probe(compose, run=subprocess.run):
    for binary in ('attune', 'attune-mcp'):
        for empty_roots in (False, True):
            try:
                result = run(command(compose, binary, empty_roots),
                             capture_output=True, timeout=30, check=False)
                passed = check(result, binary, empty_roots)
                code = result.returncode
            except (OSError, subprocess.TimeoutExpired):
                passed, code = False, None
            print(json.dumps({'probe': 'packaged_client_roots', 'binary': binary,
                              'check': 'empty_roots_denied' if empty_roots else 'api_initialized',
                              'passed': passed, 'exit': code}), flush=True)
            if not passed:
                # The owning runner retains responsibility for project cleanup;
                # a timeout/transport error never counts as root denial.
                return 1
    return 0


if __name__ == '__main__':
    if len(sys.argv) < 3 or sys.argv[1] != '--':
        raise SystemExit('usage: probe_packaged_client_roots.py -- docker compose <owned project options>')
    raise SystemExit(probe(sys.argv[2:]))
