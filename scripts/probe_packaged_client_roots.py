#!/usr/bin/env python3
"""Real packaged clients: remote info plus the distinct HTTP caller startup.

The owning runner supplies its already-built init-agent image and healthy API.
CLI/stdin-MCP info may use bundled roots; HTTP callers deliberately do not use
that fallback. Only bounded static results reach diagnostics, never child logs.
"""
import json
import re
import subprocess
import sys
import time
import uuid

API = 'http://api:8080'
MISSING_ROOTS = b'No CA certificates were loaded from the system'
CALLER_FAILURE = b'Could not initialize caller API client'
OWNER_LABEL = 'io.attune.packaged-client-probe'
MAX_OUTPUT = 65536


class ProbeFailure(Exception):
    """Codes are fixed by this driver; no child output/command is included."""


def invoke(run, args, timeout=10):
    try:
        result = run(args, capture_output=True, timeout=timeout, check=False)
    except subprocess.TimeoutExpired:
        raise ProbeFailure('command_timeout') from None
    except OSError:
        raise ProbeFailure('command_unavailable') from None
    if len(result.stdout) + len(result.stderr) > MAX_OUTPUT:
        raise ProbeFailure('output_limit')
    return result


def checked(run, args, timeout=10):
    result = invoke(run, args, timeout)
    if result.returncode != 0:
        raise ProbeFailure('command_failed')
    return result.stdout


def command(compose, binary):
    # --info --local is deliberately absent: it constructs no API client.
    args = [*compose, 'run', '--rm', '--no-deps', '-T',
            '--entrypoint', '/usr/local/bin/' + binary, '-e', 'HOME=/tmp',
            'init-agent', '--api-url', API]
    return args + (['--output', 'json', 'info'] if binary == 'attune' else ['--info'])


def check_info(result, binary):
    if result.returncode != 0:
        return False
    try:
        report = json.loads(result.stdout)
        return (report['local']['binary'] == binary
                and report['server']['status'] == 'connected'
                and report['local']['version'] == report['server']['version'])
    except (ValueError, TypeError, KeyError):
        return False


def project_name(compose):
    for flag in ('--project-name', '-p'):
        if flag in compose:
            index = compose.index(flag) + 1
            if index < len(compose) and re.fullmatch(r'[a-z0-9][a-z0-9_-]*', compose[index]):
                return compose[index]
    raise ProbeFailure('owned_project_required')


def image_id(compose, run):
    selected = checked(run, [*compose, 'images', '--quiet', 'init-agent']).strip()
    if not re.fullmatch(rb'(?:sha256:)?[0-9a-f]{12,64}', selected):
        raise ProbeFailure('image_identity')
    full = checked(run, ['docker', 'image', 'inspect', '--format', '{{.Id}}', selected.decode()]).strip()
    if not re.fullmatch(rb'sha256:[0-9a-f]{64}', full):
        raise ProbeFailure('image_identity')
    return full.decode()


class McpHttpProbe:
    """One fixed, owned test container; no general launch/configuration API."""
    def __init__(self, image, project, run, now=time.monotonic, pause=time.sleep):
        self.image, self.project, self.run = image, project, run
        self.now, self.pause = now, pause
        self.owner = uuid.uuid4().hex
        self.name = 'attune-mcp-ca-probe-' + self.owner
        self.identifier = None
        self.verified = False
        self.submitted = False
        self.cleanup_facts = {
            'cleanup_failed_step': None,
            'cleanup_failure_reason': None,
            'cleanup_wait_exit': None,
            'cleanup_observed_exit': None,
            'cleanup_running': None,
        }

    def docker(self, *args, timeout=10):
        return checked(self.run, ['docker', *args], timeout)

    def inspect_owned(self):
        reference = self.identifier or self.name
        raw = self.docker('inspect', '--format', '{{json .}}', reference, timeout=3)
        try:
            value = json.loads(raw)
            labels = value['Config']['Labels']
            identifier = value['Id']
            state = value['State']
            valid = (re.fullmatch(r'[0-9a-f]{64}', identifier)
                     and value['Name'] == '/' + self.name
                     and value['Image'] == self.image
                     and isinstance(labels, dict) and isinstance(state, dict)
                     and labels.get(OWNER_LABEL) == self.owner
                     and labels.get('com.docker.compose.project') == self.project
                     and (self.identifier is None or identifier == self.identifier)
                     and all(type(state[key]) is bool for key in ('Running', 'Restarting', 'OOMKilled'))
                     and type(state['ExitCode']) is int and 0 <= state['ExitCode'] <= 255)
        except (ValueError, TypeError, KeyError):
            raise ProbeFailure('resource_ownership') from None
        if not valid:
            raise ProbeFailure('resource_ownership')
        self.identifier = identifier
        self.verified = True
        return value

    def create_and_start(self, empty_roots):
        args = ['create', '--pull', 'never', '--name', self.name,
                '--label', OWNER_LABEL + '=' + self.owner,
                '--label', 'com.docker.compose.project=' + self.project,
                '--label', 'com.docker.compose.service=init-agent',
                '--label', 'com.docker.compose.oneoff=True',
                '--network', 'none', '--restart', 'no', '--no-healthcheck',
                '--user', '1000:1000', '--read-only', '--cap-drop', 'ALL',
                '--security-opt', 'no-new-privileges:true',
                '--tmpfs', '/tmp:rw,nosuid,nodev,noexec,size=1048576,mode=1777',
                '--entrypoint', '/usr/local/bin/attune-mcp', '-e', 'HOME=/tmp']
        if empty_roots:
            args += ['-e', 'SSL_CERT_FILE=/dev/null',
                     '-e', 'SSL_CERT_DIR=/attune-owned-probe-no-certificates']
        args += [self.image, '--transport', 'http', '--api-url', 'http://127.0.0.1:1',
                 '--listen-addr', '127.0.0.1:8090', '--http-host', 'localhost:8090']
        # A lost create/start ACK still enters finally and exact owned cleanup.
        self.submitted = True
        identifier = self.docker(*args).strip()
        if not re.fullmatch(rb'[0-9a-f]{64}', identifier):
            raise ProbeFailure('resource_identity')
        self.identifier = identifier.decode()
        self.inspect_owned()
        if self.docker('start', self.identifier).strip() != identifier:
            raise ProbeFailure('start_ack')

    def observe(self, empty_roots):
        deadline = self.now() + 20
        while self.now() < deadline:
            value = self.inspect_owned()
            state = value['State']
            if state.get('Running') is False:
                code = self.docker('wait', self.identifier).strip()
                if not re.fullmatch(rb'[0-9]{1,3}', code):
                    raise ProbeFailure('wait_ack')
                code = int(code)
                captured = invoke(self.run, ['docker', 'logs', self.identifier])
                if captured.returncode != 0:
                    raise ProbeFailure('command_failed')
                logs = captured.stdout + captured.stderr
                # Require the actual caller constructor failure, not info's
                # fallback, a timeout, a kill, OOM, or an unrelated CLI error.
                passed = (empty_roots and code == 1 and state.get('ExitCode') == code
                          and state.get('OOMKilled') is False
                          and CALLER_FAILURE in logs and MISSING_ROOTS in logs)
                return passed, code, None
            if state.get('Running') is not True:
                raise ProbeFailure('process_state')
            if not empty_roots:
                health = invoke(self.run, ['docker', 'exec', self.identifier,
                    '/bin/wget', '-q', '-S', '-T', '1', '-O', '-',
                    '--header', 'Host: localhost:8090', 'http://127.0.0.1:8090/health'], timeout=3)
                statuses = re.findall(rb'HTTP/1\.[01] ([0-9]{3})(?:\s|$)', health.stderr)
                if health.returncode == 0 and statuses == [b'200'] and health.stdout == b'':
                    return True, None, 200
            self.pause(0.1)
        raise ProbeFailure('startup_deadline')

    def cleanup(self):
        if not self.submitted:
            return True
        # Same owning-fixture order: exact identity -> Stop -> Wait -> stopped
        # inspection -> nonforced Remove ACK. Never infer removal from 404.
        acknowledged = True
        def fail(step, reason):
            nonlocal acknowledged
            acknowledged = False
            if self.cleanup_facts['cleanup_failed_step'] is None:
                self.cleanup_facts['cleanup_failed_step'] = step
                self.cleanup_facts['cleanup_failure_reason'] = reason

        try:
            self.inspect_owned()
        except ProbeFailure as failure:
            fail('inspect_before_stop', str(failure))
            if not self.verified or str(failure) == 'resource_ownership':
                return False
            # A start timeout may leave a running daemon. Its already verified
            # immutable ID still permits Stop/Wait/Remove after a read failure;
            # the unknown inspection ACK remains a failed probe.
        waited = None
        try:
            if self.docker('stop', '--time', '5', self.identifier).strip() != self.identifier.encode():
                fail('stop', 'ack_mismatch')
        except ProbeFailure as failure:
            fail('stop', str(failure))
        try:
            raw = self.docker('wait', self.identifier).strip()
            waited = int(raw) if re.fullmatch(rb'[0-9]{1,3}', raw) else None
            self.cleanup_facts['cleanup_wait_exit'] = waited
            if waited is None:
                fail('wait', 'ack_mismatch')
        except ProbeFailure as failure:
            fail('wait', str(failure))
        try:
            state = self.inspect_owned()['State']
            self.cleanup_facts['cleanup_observed_exit'] = state.get('ExitCode')
            self.cleanup_facts['cleanup_running'] = state.get('Running')
            if not (state.get('Running') is False and state.get('Restarting') is False
                    and waited is not None and state.get('ExitCode') == waited):
                fail('inspect_after_wait', 'state_mismatch')
        except ProbeFailure as failure:
            fail('inspect_after_wait', str(failure))
        except (KeyError, TypeError):
            fail('inspect_after_wait', 'state_mismatch')
        try:
            # Still attempt nonforced removal after uncertain Stop/Wait: it
            # cannot delete a running container. The uncertain case stays FAIL.
            if self.docker('rm', self.identifier).strip() != self.identifier.encode():
                fail('remove', 'ack_mismatch')
        except ProbeFailure as failure:
            fail('remove', str(failure))
        return bool(acknowledged)

    def probe(self, empty_roots):
        passed, code, status, error = False, None, None, None
        try:
            self.create_and_start(empty_roots)
            passed, code, status = self.observe(empty_roots)
            if not passed:
                error = 'unexpected_caller_result'
        except ProbeFailure as failure:
            error = str(failure)
        finally:
            cleanup = self.cleanup()
        return {'probe': 'packaged_client_roots', 'binary': 'attune-mcp',
                'check': 'empty_caller_roots_denied' if empty_roots else 'http_caller_initialized',
                'passed': passed and cleanup, 'exit': code, 'http': status,
                'cleanup_ack': cleanup, 'error': error if cleanup else 'cleanup_unknown',
                **self.cleanup_facts}


def probe(compose, run=subprocess.run, now=time.monotonic, pause=time.sleep):
    for binary in ('attune', 'attune-mcp'):
        code = None
        try:
            result = invoke(run, command(compose, binary), timeout=30)
            passed, code = check_info(result, binary), result.returncode
        except ProbeFailure:
            passed = False
        print(json.dumps({'probe': 'packaged_client_roots', 'binary': binary,
                          'check': 'api_initialized', 'passed': passed, 'exit': code}), flush=True)
        if not passed:
            return 1
    try:
        project, image = project_name(compose), image_id(compose, run)
        for empty_roots in (False, True):
            result = McpHttpProbe(image, project, run, now, pause).probe(empty_roots)
            print(json.dumps(result), flush=True)
            if not result['passed']:
                return 1
    except ProbeFailure:
        print(json.dumps({'probe': 'packaged_client_roots', 'check': 'owned_image_required',
                          'passed': False}), flush=True)
        return 1
    return 0


if __name__ == '__main__':
    if len(sys.argv) < 3 or sys.argv[1] != '--':
        raise SystemExit('usage: probe_packaged_client_roots.py -- docker compose <owned project options>')
    raise SystemExit(probe(sys.argv[2:]))
