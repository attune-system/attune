"""Pure ownership/manifest checks for the artifact benchmark runner."""

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

SCRIPT = Path(__file__).with_name("benchmark-artifact-reads.py")
spec = importlib.util.spec_from_file_location("artifact_benchmark", SCRIPT)
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)


class ArtifactBenchmarkRunnerTests(unittest.TestCase):
    def test_paired_reference_selection_enforces_targets_instead_of_only_labeling_the_run(self):
        class Server:
            def __init__(self, image, run_id, case):
                self.case, self.name = case, 'owned'
            def __enter__(self):
                self.case['port'] = '127.0.0.1:12345'
                return self
            def __exit__(self, *args):
                self.case['cleanup_errors'] = []
            def sql(self, _):
                return ''
        protocol = SimpleNamespace(Server=Server, command=lambda *args: SimpleNamespace(stdout='owned fixture df'))
        for seconds, expected in [(.1,0),(.9,1)]:
            with self.subTest(seconds=seconds), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                output = root / 'evidence'
                def run(args, **kwargs):
                    if args[0] == 'git':
                        return subprocess.CompletedProcess(args,0,'revision\n','')
                    if args[-1] == '--list':
                        return subprocess.CompletedProcess(args,0,'reference::paired_serial_reference_latency_and_query_gate: test\n','')
                    if '--ignored' in args:
                        sample = dict(comparison_kind='controlled-serial-reference-not-historical-revision',
                            identical_response_bodies=True, query_reduction_target=.90,p95_reduction_target=.70,
                            fixture='paired',permissions='same',cache_state='warm',concurrency=1,completed_requests=20,
                            baseline=[dict(status=200,handler_seconds=1,queries=dict(statements=807)) for _ in range(20)],
                            treatment=[dict(status=200,handler_seconds=seconds,queries=dict(statements=10)) for _ in range(20)])
                        (Path(kwargs['env']['ATTUNE_ARTIFACT_BENCHMARK_OUTPUT'])/'paired-serial-reference.json').write_text(json.dumps(sample))
                    return subprocess.CompletedProcess(args,0)
                with patch.object(benchmark,'ROOT',root), \
                        patch.object(benchmark.subprocess,'run',side_effect=run), \
                        patch.object(benchmark,'compile_test',return_value='test-executable'), \
                        patch.object(benchmark,'load_protocol',return_value=protocol), \
                        patch('sys.argv',[str(SCRIPT),'--output',str(output),'--versions','18','--paired-reference']), \
                        contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(benchmark.main(),expected)
                evidence = json.loads((output/'evidence.json').read_text())
                self.assertEqual(evidence['passed'],expected==0)
                self.assertEqual(evidence['cases'][0]['paired_reference_acceptance']['passed'],expected==0)

    def test_owned_database_removal_has_a_bounded_docker_teardown_budget(self):
        protocol = benchmark.load_protocol()
        server = protocol.Server('postgres:18-alpine', 'owned-test', {})
        server.container_claimed = server.volume_owned = True
        info = {'Id':'owned-id','Config':{'Labels':{protocol.LABEL:server.run_id}},
                'Labels':{protocol.LABEL:server.run_id}}
        result = subprocess.CompletedProcess([], 0, json.dumps([info]), '')
        with patch.object(protocol, 'command', return_value=result) as command:
            server.cleanup()
        removals = [call for call in command.call_args_list if call.args[:2] == ('docker','rm')
                    or call.args[:3] == ('docker','volume','rm')]
        self.assertEqual(len(removals), 2)
        self.assertTrue(all(call.kwargs['timeout'] == 180 for call in removals))
        self.assertEqual(server.report['cleanup_errors'], [])

    def test_frozen_build_is_isolated_and_missing_selected_tests_abort_before_database_setup(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            root = parent / 'checkout'
            root.mkdir()
            (root / 'Cargo.lock').write_text('fixture lock')
            executable = parent / 'test-executable'
            executable.write_bytes(b'fixture executable')
            output = parent / 'evidence'
            def run(args, **kwargs):
                return subprocess.CompletedProcess(args, 0, 'revision\n' if args[0] == 'git' else '', '')
            with patch.object(benchmark, 'ROOT', root), \
                    patch.object(benchmark.subprocess, 'run', side_effect=run), \
                    patch.object(benchmark, 'compile_test', return_value=executable) as build, \
                    patch.object(benchmark, 'load_protocol', side_effect=AssertionError('database setup must not start')), \
                    patch.dict(benchmark.os.environ, {'CARGO_TARGET_DIR': str(parent / 'build')}), \
                    patch('sys.argv', [str(SCRIPT), '--output', str(output), '--snapshot-source', '--workload-profile']), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(benchmark.main(), 1)
            self.assertEqual(build.call_args.kwargs['target_dir'], parent / 'build/artifact-benchmarks/evidence')
            evidence = json.loads((output / 'evidence.json').read_text())
            self.assertIn('does not contain required workload test', evidence['error'])
            self.assertEqual(evidence['cases'], [])

    def test_captured_executable_is_independent_and_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'shared-executable'
            source.write_bytes(b'owned executable fixture')
            destination = root / 'pinned-executable'
            evidence = benchmark.capture_executable(source, destination)
            source.write_bytes(b'later rebuild')
            self.assertEqual(destination.read_bytes(), b'owned executable fixture')
            self.assertEqual(evidence['sha256'], benchmark.hashlib.sha256(destination.read_bytes()).hexdigest())
            self.assertEqual(destination.stat().st_mode & 0o777, 0o500)
            with self.assertRaises(FileExistsError):
                benchmark.capture_executable(source, destination)
    def test_source_capture_omits_local_credentials_and_build_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            root = parent / 'checkout'
            root.mkdir()
            (root / 'Cargo.lock').write_text('fixture lock')
            (root / '.env').write_text('private fixture value')
            (root / 'target').mkdir()
            (root / 'target' / 'large-build').write_text('build output')
            fixture = root / 'crates/common/tests/fixtures/release_v1/dist/app.jar'
            fixture.parent.mkdir(parents=True)
            fixture.write_text('required fixture')
            with patch.object(benchmark, 'ROOT', root):
                copied = benchmark.capture_sources(parent / 'snapshot', benchmark.snapshot())
                self.assertEqual(benchmark.snapshot(copied), benchmark.snapshot())
            self.assertFalse((copied / '.env').exists())
            self.assertFalse((copied / 'target').exists())
            self.assertFalse((copied / '.git').exists())
            self.assertEqual((copied / fixture.relative_to(root)).read_text(), 'required fixture')

    def test_source_capture_refuses_recursive_paths_and_mismatched_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'checkout'
            root.mkdir()
            (root / 'Cargo.lock').write_text('fixture lock')
            with patch.object(benchmark, 'ROOT', root):
                with self.assertRaises(ValueError):
                    benchmark.capture_sources(root / 'nested', benchmark.snapshot())
                with self.assertRaisesRegex(RuntimeError, 'source snapshot does not match'):
                    benchmark.capture_sources(Path(directory) / 'snapshot', {})

    def test_nested_origin_fixtures_are_part_of_the_source_fingerprint(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'crates/api/tests/artifact_read_performance/source_origins.rs'
            source.parent.mkdir(parents=True)
            source.write_text('owned fixture\n')
            with patch.object(benchmark, 'ROOT', root):
                before = benchmark.snapshot()
                source.write_text('changed fixture\n')
                after = benchmark.snapshot()
            self.assertIn(str(source.relative_to(root)), before)
            self.assertNotEqual(before, after)

    def test_dry_run_hashes_sources_without_creating_resources_or_exposing_contents(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "config.test.yaml").write_text("credential: fixture-private-value\n")
            output = root / "evidence"
            stdout = io.StringIO()
            revision = subprocess.CompletedProcess([], 0, "fixture-revision\n", "")
            with patch.object(benchmark, "ROOT", root), \
                    patch.object(benchmark.subprocess, "run", return_value=revision), \
                    patch.object(benchmark, "compile_test", side_effect=AssertionError("unexpected build")), \
                    patch.object(benchmark, "load_protocol", side_effect=AssertionError("unexpected Docker fixture")), \
                    patch("sys.argv", [str(SCRIPT), "--dry-run", "--output", str(output)]), \
                    contextlib.redirect_stdout(stdout):
                self.assertEqual(benchmark.main(), 0)
            self.assertFalse(output.exists())
            manifest = json.loads(stdout.getvalue())
            self.assertEqual(manifest["revision"], "fixture-revision")
            self.assertEqual(len(manifest["source_sha256"]["config.test.yaml"]), 64)
            self.assertNotIn("fixture-private-value", stdout.getvalue())

    def test_existing_output_is_rejected_before_building(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch("sys.argv", [str(SCRIPT), "--output", directory]), \
                patch.object(benchmark, "compile_test", side_effect=AssertionError("unexpected build")), \
                contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                benchmark.main()
            self.assertEqual(error.exception.code, 2)

    def test_cargo_artifact_selection_ignores_dependency_executables(self):
        messages = [
            {"reason": "compiler-artifact", "target": {"name": "dependency"}, "executable": "/other"},
            {"reason": "compiler-artifact", "target": {"name": benchmark.TEST}, "executable": "/owned/test"},
        ]
        result = subprocess.CompletedProcess([], 0, "\n".join(map(json.dumps, messages)), "")
        with patch.object(benchmark.subprocess, "run", return_value=result):
            self.assertEqual(benchmark.compile_test(), "/owned/test")

    def test_compile_failure_does_not_run_stale_test_executables(self):
        result = subprocess.CompletedProcess([], 1, "", "compile failed")
        with patch.object(benchmark.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "Cannot compile"):
                benchmark.compile_test()

    def test_main_records_source_changes_even_when_compilation_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'Cargo.lock'
            source.write_text('before')
            output = root / 'evidence'

            def fail_compile(*args, **kwargs):
                source.write_text('changed')
                raise RuntimeError('deliberate compile failure')

            revision = subprocess.CompletedProcess([], 0, 'revision\n', '')
            with patch.object(benchmark, 'ROOT', root), \
                    patch.object(benchmark.subprocess, 'run', return_value=revision), \
                    patch.object(benchmark, 'compile_test', side_effect=fail_compile), \
                    patch('sys.argv', [str(SCRIPT), '--output', str(output)]), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(benchmark.main(), 1)
            evidence = json.loads((output / 'evidence.json').read_text())
            self.assertFalse(evidence['passed'])
            self.assertFalse(evidence['source_unchanged'])
            self.assertEqual(evidence['source_changes'], ['Cargo.lock'])
            self.assertEqual(evidence['cases'], [])

    def test_profile_has_explicit_selection_unique_owners_and_requires_a_report(self):
        calls = []

        class Server:
            def __init__(self, image, run_id, case):
                self.case = case
                self.name = 'owned'

            def __enter__(self):
                self.case['port'] = '127.0.0.1:12345'
                return self

            def __exit__(self, *args):
                self.case['cleanup_errors'] = []

            def sql(self, _):
                return ''

        def run(args, **kwargs):
            if args[:2] == ['git', 'rev-parse']:
                return subprocess.CompletedProcess(args, 0, 'revision\n', '')
            if args[-1] == '--list':
                tests = ['artifact_partition_and_maintenance_workload_profile', 'artifact_cardinality_and_history_workload_profile']
                return subprocess.CompletedProcess(args, 0, '\n'.join(f'source_origins::workload::{test}: test' for test in tests), '')
            calls.append((args, dict(kwargs['env'])))
            return subprocess.CompletedProcess(args, 0)

        protocol = SimpleNamespace(Server=Server, command=lambda *args: SimpleNamespace(stdout='owned fixture df'))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / 'evidence'
            with patch.object(benchmark, 'ROOT', root), \
                    patch.object(benchmark.subprocess, 'run', side_effect=run), \
                    patch.object(benchmark, 'compile_test', side_effect=['artifact-test', 'key-test']), \
                    patch.object(benchmark, 'load_protocol', return_value=protocol), \
                    patch('sys.argv', [str(SCRIPT), '--output', str(output), '--versions', '18', '--with-key-usage-tests', '--workload-profile']), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(benchmark.main(), 1)
            evidence = json.loads((output / 'evidence.json').read_text())
            self.assertIn('did not produce its report', evidence['error'])
            self.assertEqual(len(calls), 4)
            self.assertNotIn('--ignored', calls[0][0])
            self.assertIn('--ignored', calls[2][0])
            self.assertIn('--exact', calls[2][0])
            self.assertEqual(len({env['ATTUNE_TEST_RUN_ID'] for _, env in calls}), 4)
            self.assertTrue(all('--test-threads=4' in args for args, _ in calls))


if __name__ == "__main__":
    unittest.main()
