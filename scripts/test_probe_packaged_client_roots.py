"""Wire/lifecycle/privacy checks; actual image behavior remains owned E2E evidence."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import unittest

SPEC = importlib.util.spec_from_file_location('client_roots', Path(__file__).with_name('probe_packaged_client_roots.py'))
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)
IMAGE = 'sha256:' + 'a' * 64
PRIVATE = b'fixture-private-value-must-not-be-logged'


class Clock:
    def __init__(self):
        self.value = 0

    def now(self):
        return self.value

    def pause(self, _):
        self.value += 5


class Docker:
    def __init__(self, fault=None, health=200):
        self.calls, self.containers = [], {}
        self.fault, self.health = fault, health
        self.last = None
        self.logs_seen = False
        self.health_seen = False
        self.inspect_failed = False

    def __call__(self, args, **kwargs):
        self.calls.append(args)
        assert kwargs == dict(capture_output=True, timeout=kwargs['timeout'], check=False)
        assert 0 < kwargs['timeout'] <= 30
        result = lambda code=0, out=b'', err=b'': subprocess.CompletedProcess(args, code, out, err)
        if args[:2] == ['docker', 'compose']:
            if 'images' in args:
                return result(out=b'a' * 64 + b'\n')
            binary = args[args.index('--entrypoint') + 1].rsplit('/', 1)[1]
            report = {'local': {'binary': binary, 'version': 'candidate'},
                      'server': {'status': 'connected', 'version': 'candidate'}}
            return result(out=json.dumps(report).encode())
        op = args[1]
        if op == 'image':
            return result(out=IMAGE.encode() + b'\n')
        if op == 'create':
            labels = dict(args[index + 1].split('=', 1) for index, arg in enumerate(args) if arg == '--label')
            identifier = str(len(self.containers) + 1) * 64
            value = {'Id': identifier, 'Name': '/' + args[args.index('--name') + 1],
                     'Image': IMAGE, 'Config': {'Labels': labels},
                     'empty': 'SSL_CERT_FILE=/dev/null' in args,
                     'cleanup': False,
                     'State': {'Running': False, 'Restarting': False, 'OOMKilled': False, 'ExitCode': 0}}
            self.containers[identifier] = self.last = value
            if self.fault == 'foreign':
                labels[probe.OWNER_LABEL] = 'foreign-owner'
            if self.fault == 'create_timeout':
                raise subprocess.TimeoutExpired(PRIVATE, 10, PRIVATE)
            return result(out=identifier.encode() + b'\n')
        reference = args[-1]
        if op == 'inspect':
            if self.fault == 'cleanup_inspect_timeout' and (self.logs_seen or self.health_seen) and not self.inspect_failed:
                self.inspect_failed = True
                raise subprocess.TimeoutExpired(PRIVATE, 3, PRIVATE)
            value = next((v for v in self.containers.values() if reference in (v['Id'], v['Name'][1:])), None)
            if value and value['cleanup'] and self.fault in ('exit_mismatch', 'running_mismatch'):
                state = dict(value['State'])
                if self.fault == 'exit_mismatch':
                    state['ExitCode'] += 1
                else:
                    state['Running'] = True
                value = dict(value, State=state)
            return result(out=json.dumps(value).encode()) if value else result(1, err=PRIVATE)
        value = self.containers.get(reference) if op != 'exec' else self.containers.get(args[2])
        assert value is not None, ('unexpected/foreign reference', args)
        if op == 'start':
            value['State'].update(Running=not value['empty'] or self.fault == 'never_exits',
                                  ExitCode=0 if self.fault == 'child_success' else 1 if value['empty'] else 0)
            if self.fault == 'start_timeout':
                raise subprocess.TimeoutExpired(PRIVATE, 10, PRIVATE)
            return result(out=reference.encode() + b'\n')
        if op == 'exec':
            self.health_seen = True
            return result(0 if self.health == 200 else 1,
                          err=f'  HTTP/1.1 {self.health} response\n'.encode())
        if op == 'logs':
            self.logs_seen = True
            logs = PRIVATE if self.fault == 'wrong_error' else probe.CALLER_FAILURE + b': ' + probe.MISSING_ROOTS + PRIVATE
            if self.fault == 'output_limit':
                logs += b'x' * probe.MAX_OUTPUT
            # Docker forwards container stderr on CLI stderr, not stdout.
            return result(err=logs)
        if op == 'stop':
            value['cleanup'] = True
            if value['State']['Running']:
                value['State'].update(Running=False, ExitCode=137)
            return result(1 if self.fault in ('stop_ack', 'stop_and_wait_ack') else 0,
                          out=PRIVATE if self.fault == 'stop_echo' else reference.encode() + b'\n')
        if op == 'wait':
            assert value['State']['Running'] is False
            if value['cleanup'] and self.fault in ('wait_ack', 'stop_and_wait_ack'):
                raise subprocess.TimeoutExpired(PRIVATE, 10, PRIVATE)
            if value['cleanup'] and self.fault == 'wait_echo':
                return result(out=PRIVATE)
            return result(out=str(value['State']['ExitCode']).encode() + b'\n')
        if op == 'rm':
            assert not value['State']['Running']
            del self.containers[reference]
            if self.fault == 'remove_ack':
                raise subprocess.TimeoutExpired(PRIVATE, 10, PRIVATE)
            return result(out=PRIVATE if self.fault == 'remove_echo' else reference.encode() + b'\n')
        raise AssertionError(('unexpected command', args))


class PackagedClientRootsTests(unittest.TestCase):
    def http(self, docker, empty=True):
        clock = Clock()
        return probe.McpHttpProbe(IMAGE, 'owned', docker, clock.now, clock.pause).probe(empty)

    def test_real_info_positives_then_distinct_http_caller_controls(self):
        docker, clock = Docker(), Clock()
        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(probe.probe(['docker', 'compose', '-p', 'owned'], docker, clock.now, clock.pause), 0)
        records = list(map(json.loads, output.getvalue().splitlines()))
        self.assertEqual([r['check'] for r in records], ['api_initialized', 'api_initialized',
                         'http_caller_initialized', 'empty_caller_roots_denied'])
        self.assertTrue(all(row['passed'] for row in records))
        self.assertEqual(records[2]['http'], 200)
        self.assertEqual(records[3]['exit'], 1)
        self.assertTrue(all(row['cleanup_ack'] for row in records[2:]))
        for record, code in zip(records[2:], (137, 1)):
            self.assertIsNone(record['cleanup_failed_step'])
            self.assertIsNone(record['cleanup_failure_reason'])
            self.assertEqual(record['cleanup_wait_exit'], code)
            self.assertEqual(record['cleanup_observed_exit'], code)
            self.assertIs(record['cleanup_running'], False)
        self.assertNotIn(PRIVATE.decode(), output.getvalue())
        self.assertEqual(docker.containers, {})
        infos = [args for args in docker.calls if '--entrypoint' in args and 'run' in args]
        self.assertEqual(len(infos), 2)
        self.assertTrue(all('--local' not in args and '--rm' in args and '--no-deps' in args
                            and 'SSL_CERT_FILE=/dev/null' not in args for args in infos))
        creates = [args for args in docker.calls if args[1] == 'create']
        self.assertEqual(len(creates), 2)
        self.assertEqual(sum('SSL_CERT_FILE=/dev/null' in args for args in creates), 1)
        for args in creates:
            self.assertEqual(args[args.index('--network') + 1], 'none')
            self.assertEqual(args[args.index('--listen-addr') + 1], '127.0.0.1:8090')
            self.assertEqual(args[args.index('--http-host') + 1], 'localhost:8090')
            for forbidden in ('--publish', '-p', '--auth-token', '--execution-token', '--login', '--password', '--profile', '--info'):
                self.assertNotIn(forbidden, args)
        health = next(args for args in docker.calls if args[1] == 'exec')
        self.assertEqual(health[health.index('--header') + 1], 'Host: localhost:8090')
        self.assertEqual(health[-1], 'http://127.0.0.1:8090/health')
        for identifier in {args[-1] for args in docker.calls if args[1] == 'rm'}:
            ops = [args[1] for args in docker.calls if args[-1] == identifier]
            self.assertEqual(ops[-4:], ['stop', 'wait', 'inspect', 'rm'])

    def test_info_fallback_success_is_not_misclassified_as_missing_roots_failure(self):
        report = {'local': {'binary': 'attune', 'version': 'candidate'},
                  'server': {'status': 'connected', 'version': 'candidate'}}
        self.assertTrue(probe.check_info(subprocess.CompletedProcess([], 0, json.dumps(report).encode(), probe.MISSING_ROOTS), 'attune'))
        self.assertFalse(probe.check_info(subprocess.CompletedProcess([], 1, b'', PRIVATE), 'attune'))
        self.assertFalse(probe.check_info(subprocess.CompletedProcess([], 0, b'{"server":{"status":"unavailable"}}', b''), 'attune'))

    def test_wrong_failure_success_and_running_child_are_not_empty_store_denial(self):
        for fault in ('wrong_error', 'child_success', 'never_exits', 'output_limit'):
            with self.subTest(fault=fault):
                docker = Docker(fault)
                result = self.http(docker)
                self.assertFalse(result['passed'])
                self.assertTrue(result['cleanup_ack'])
                self.assertEqual(docker.containers, {})
                self.assertNotIn(PRIVATE.decode(), json.dumps(result))

    def test_create_and_start_timeout_still_join_remove_but_never_pass(self):
        for fault in ('create_timeout', 'start_timeout'):
            with self.subTest(fault=fault):
                docker = Docker(fault)
                result = self.http(docker)
                self.assertFalse(result['passed'])
                self.assertTrue(result['cleanup_ack'])
                self.assertEqual(result['error'], 'command_timeout')
                self.assertEqual(docker.containers, {})
                self.assertTrue(any(args[1] == 'stop' for args in docker.calls))
                self.assertTrue(any(args[1] == 'wait' for args in docker.calls))
                self.assertTrue(any(args[1] == 'rm' for args in docker.calls))

    def test_unknown_cleanup_fails_even_after_matching_caller_error(self):
        faults = {
            'stop_ack': ('stop', 'command_failed'),
            'wait_ack': ('wait', 'command_timeout'),
            'remove_ack': ('remove', 'command_timeout'),
            'cleanup_inspect_timeout': ('inspect_before_stop', 'command_timeout'),
            'stop_echo': ('stop', 'ack_mismatch'),
            'wait_echo': ('wait', 'ack_mismatch'),
            'remove_echo': ('remove', 'ack_mismatch'),
            'exit_mismatch': ('inspect_after_wait', 'state_mismatch'),
            'running_mismatch': ('inspect_after_wait', 'state_mismatch'),
            'stop_and_wait_ack': ('stop', 'command_failed'),
        }
        for empty in (False, True):
            for fault, (step, reason) in faults.items():
                with self.subTest(fault=fault, empty=empty):
                    docker = Docker(fault)
                    result = self.http(docker, empty)
                    self.assertFalse(result['passed'])
                    self.assertFalse(result['cleanup_ack'])
                    self.assertEqual(result['error'], 'cleanup_unknown')
                    self.assertEqual(result['cleanup_failed_step'], step)
                    self.assertEqual(result['cleanup_failure_reason'], reason)
                    code = 1 if empty else 137
                    self.assertEqual(result['cleanup_wait_exit'], None if fault in ('wait_ack', 'wait_echo', 'stop_and_wait_ack') else code)
                    self.assertEqual(result['cleanup_observed_exit'], code + (fault == 'exit_mismatch'))
                    self.assertIs(result['cleanup_running'], fault == 'running_mismatch')
                    self.assertEqual(docker.containers, {})
                    self.assertNotIn(PRIVATE.decode(), json.dumps(result))
                    self.assertFalse(any(name in result for name in ('id', 'identifier', 'owner', 'labels', 'stdout', 'stderr', 'exception')))
                    # Later wait/state failures cannot replace the first stop
                    # failure, and every cleanup operation is still attempted.
                    for op in ('stop', 'wait', 'rm'):
                        self.assertTrue(any(args[1] == op for args in docker.calls))
                    ops = [args[1] for args in docker.calls if args[-1] == docker.last['Id']]
                    self.assertEqual(ops[-4:], ['stop', 'wait', 'inspect', 'rm'])

    def test_foreign_resource_is_never_stopped_or_removed(self):
        docker = Docker('foreign')
        result = self.http(docker)
        self.assertFalse(result['passed'])
        self.assertFalse(result['cleanup_ack'])
        self.assertEqual(result['cleanup_failed_step'], 'inspect_before_stop')
        self.assertEqual(result['cleanup_failure_reason'], 'resource_ownership')
        self.assertFalse(any(args[1] in ('stop', 'wait', 'rm') for args in docker.calls))

    def test_health_requires_exact_200_and_timeout_still_cleans_up(self):
        for status in (201, 301, 401, 403, 500):
            with self.subTest(status=status):
                docker = Docker(health=status)
                result = self.http(docker, empty=False)
                self.assertFalse(result['passed'])
                self.assertTrue(result['cleanup_ack'])
                self.assertEqual(result['error'], 'startup_deadline')
                self.assertEqual(docker.containers, {})


if __name__ == '__main__':
    unittest.main()
