#!/usr/bin/env python3
"""Tool-boundary regressions using synthetic child output; no product is executed."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

TOOLS = Path(__file__).resolve().parents[4] / 'tools/namespace-adoption-bench'
sys.path.insert(0, str(TOOLS))
import artifacts


def load_tool(name):
    spec = importlib.util.spec_from_file_location(f'namespace_bench_{name}', TOOLS / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


runner = load_tool('run')
summarize = load_tool('summarize')
ENVIRONMENT = dict(GITHUB_RUN_ID='1234', GITHUB_RUN_ATTEMPT='1', GITHUB_SHA='a' * 40,
                   ZXS_EXPECTED_MANIFEST_SHA256='b' * 64, ZXS_BUILD_RESULT='success')


class RunnerBoundaryTests(unittest.TestCase):
    def phase_with_output(self, root, phase, rows):
        evidence = root / 'evidence'
        evidence.mkdir()
        payload = ''.join(json.dumps(row) + '\n' for row in rows).encode()

        def launch(command, stdout, stderr, **options):
            stdout.write(payload)
            stdout.flush()
            return SimpleNamespace(pid=123, poll=lambda: 0, wait=lambda: 0)

        with mock.patch.object(runner.subprocess, 'Popen', side_effect=launch):
            return runner.run_phase(Path('/synthetic/binary'), phase, 500, root,
                                    evidence, False, runner.time.time() + 60)

    def test_wrong_stdout_phase_is_rejected_with_raw_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(RuntimeError, 'invalid benchmark output'):
                self.phase_with_output(root, 'build', [dict(phase='open', files=500)])
            record = json.loads((root / 'evidence/build.json').read_text())
            self.assertEqual((record['status'], record['reason']), ('failed', 'output_not_validated'))
            self.assertIn('"phase": "open"', (root / 'evidence/build.stdout').read_text())

    def test_read_stdout_order_and_files_are_strict(self):
        cases = ([dict(phase='read_warm_repeat', files=500), dict(phase='read_after_open', files=500)],
                 [dict(phase='read_after_open', files=1500), dict(phase='read_warm_repeat', files=500)])
        for rows in cases:
            with self.subTest(rows=rows), tempfile.TemporaryDirectory() as temporary:
                with self.assertRaisesRegex(RuntimeError, 'invalid benchmark output'):
                    self.phase_with_output(Path(temporary), 'read', rows)

    def test_valid_read_has_two_fresh_rows(self):
        with tempfile.TemporaryDirectory() as temporary:
            result = self.phase_with_output(Path(temporary), 'read',
                                            [dict(phase=phase, files=500) for phase in
                                             ('read_after_open', 'read_warm_repeat')])
            self.assertEqual(result['status'], 'completed')
            self.assertEqual(len(result['measurements']), 2)

    def test_planned_pairs_preserve_order_trace_and_unrun_phases(self):
        runs = runner.planned_runs([500, 1500, 3000], range(4))
        self.assertEqual(len(runs), 24)
        for size_index, files in enumerate((500, 1500, 3000)):
            for pair in range(4):
                pair_runs = runs[size_index * 8 + pair * 2:size_index * 8 + pair * 2 + 2]
                expected = ['baseline', 'candidate'] if pair % 2 == 0 else ['candidate', 'baseline']
                self.assertEqual([run['implementation'] for run in pair_runs], expected)
                for run in pair_runs:
                    self.assertEqual((run['files'], run['repetition'], run['traced']), (files, pair, pair == 3))
                    self.assertEqual(run['status'], 'not_run')
                    self.assertEqual(run['not_run_phases'], list(runner.PHASES))

    def arguments(self, root, deadline=None):
        bundle = root / 'bundle'
        bundle.mkdir()
        (bundle / 'manifest.json').write_text('{}\n')
        binaries = {}
        for label in ('baseline', 'candidate'):
            binaries[label] = root / label
            binaries[label].write_bytes(label.encode())
        args = SimpleNamespace(bundle_dir=bundle, audit_dir=root / 'audit', output=root / 'results',
                               pool_parent=root / 'pools', deadline_unix=deadline if deadline is not None else runner.time.time() + 60,
                               sizes=[500], pair_index=0)

        def verify(*unused):
            args.audit_dir.mkdir()
            (args.audit_dir / 'identity-verification.json').write_text('{"status":"verified"}\n')
            return binaries

        return args, verify

    def test_failed_first_implementation_still_measures_counterpart(self):
        with tempfile.TemporaryDirectory() as temporary:
            args, verify = self.arguments(Path(temporary))
            observed = []

            def measure(run, binary, arguments, checkpoint):
                observed.append(run['implementation'])
                run['status'] = 'failed' if run['implementation'] == 'baseline' else 'completed'
                checkpoint()

            with mock.patch.object(runner, 'parse_arguments', return_value=args), \
                    mock.patch.object(runner, 'verify_bundle', side_effect=verify), \
                    mock.patch.object(runner, 'measure_implementation', side_effect=measure):
                self.assertEqual(runner.main(), 1)
            self.assertEqual(observed, ['baseline', 'candidate'])
            summary = json.loads((args.output / 'summary.json').read_text())
            self.assertEqual(summary['status'], 'failed')
            self.assertEqual([run['status'] for run in summary['runs']], ['failed', 'completed'])

    def test_expired_deadline_is_incomplete_and_launches_neither_child(self):
        with tempfile.TemporaryDirectory() as temporary:
            args, verify = self.arguments(Path(temporary), deadline=0)
            with mock.patch.object(runner, 'parse_arguments', return_value=args), \
                    mock.patch.object(runner, 'verify_bundle', side_effect=verify), \
                    mock.patch.object(runner, 'run_phase') as phase:
                self.assertEqual(runner.main(), 2)
            phase.assert_not_called()
            summary = json.loads((args.output / 'summary.json').read_text())
            self.assertEqual(summary['status'], 'incomplete')
            self.assertEqual([run['reason'] for run in summary['runs']], ['global_time_budget'] * 2)
            self.assertTrue(all(run['not_run_phases'] == list(runner.PHASES) for run in summary['runs']))

    def test_phase_exception_preserves_attempted_and_not_run_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(output=root / 'results', pool_parent=root / 'pools',
                                   deadline_unix=runner.time.time() + 60)
            args.output.mkdir()
            args.pool_parent.mkdir()
            run = runner.planned_runs([500], [0])[0]
            with mock.patch.object(runner.shutil, 'disk_usage', return_value=SimpleNamespace(free=100 * runner.GIB)), \
                    mock.patch.object(runner, 'run_phase', side_effect=RuntimeError('synthetic malformed child output')):
                runner.measure_implementation(run, Path('/synthetic/binary'), args, mock.Mock())
            record = json.loads((args.output / run['name'] / 'run.json').read_text())
            self.assertEqual(record['status'], 'failed')
            self.assertEqual(record['attempted_phases'], ['build'])
            self.assertEqual(record['not_run_phases'], ['edit', 'read', 'open', 'verify'])
            self.assertIn('synthetic malformed child output',
                          (args.output / run['name'] / 'measurement.error.txt').read_text())


class SummaryBoundaryTests(unittest.TestCase):
    def pair_summary(self, files, pair):
        order = ['baseline', 'candidate'] if pair % 2 == 0 else ['candidate', 'baseline']
        hashes = dict(baseline='c' * 64, candidate='d' * 64)
        runs = []
        for label in order:
            phases = []
            for phase in runner.PHASES:
                if phase == 'verify':
                    rows = [dict(phase='verify', files=files, receipts=22, unique_versions=files + 20)]
                else:
                    names = ('read_after_open', 'read_warm_repeat') if phase == 'read' else (phase,)
                    rows = [dict(phase=name, files=files, wall_ns=100 if label == 'baseline' else 90) for name in names]
                phases.append(dict(phase=phase, status='completed', returncode=0, measurements=rows))
            runs.append(dict(implementation=label, files=files, repetition=pair, traced=pair == 3,
                             binary_sha256=hashes[label], status='completed', phases=phases,
                             attempted_phases=list(runner.PHASES), not_run_phases=[]))
        return dict(files=files, sizes=[files], pair_index=pair, order=order,
                    github_run_id=ENVIRONMENT['GITHUB_RUN_ID'], github_run_attempt='1',
                    carrier_sha=ENVIRONMENT['GITHUB_SHA'],
                    artifact_manifest_sha256=ENVIRONMENT['ZXS_EXPECTED_MANIFEST_SHA256'],
                    binary_sha256=hashes, status='completed_measurements', runs=runs)

    def aggregate(self, root, omit=None, incomplete=None):
        source = root / 'input'
        source.mkdir()
        for files in (500, 1500, 3000):
            for pair in range(4):
                if (files, pair) == omit:
                    continue
                directory = source / f'namespace-pair-{files}-{pair}-1' / 'results'
                directory.mkdir(parents=True)
                summary = self.pair_summary(files, pair)
                if (files, pair) == incomplete:
                    summary['status'] = 'incomplete'
                    summary['runs'][0].update(status='incomplete', reason='phase_time_budget')
                (directory / 'summary.json').write_text(json.dumps(summary))
                if pair == 3:
                    for label in ('baseline', 'candidate'):
                        run_directory = directory / f'{files}-{pair}-{label}'
                        run_directory.mkdir()
                        for phase in runner.PHASES:
                            (run_directory / f'{phase}.strace').write_text('100.00 0.000001 1 3 1 fsync\n')
        destination = root / 'report'
        with mock.patch.dict(os.environ, ENVIRONMENT, clear=True), \
                mock.patch.object(sys, 'argv', ['summarize.py', '--input', str(source), '--output', str(destination)]), \
                contextlib.redirect_stdout(io.StringIO()):
            code = summarize.main()
        return code, json.loads((destination / 'summary.json').read_text())

    def test_all_twelve_pairs_accept_verify_without_wall_ns(self):
        with tempfile.TemporaryDirectory() as temporary:
            code, report = self.aggregate(Path(temporary))
        self.assertEqual(code, 0)
        self.assertEqual(report['status'], 'completed_measurements')
        self.assertEqual(len(report['pairs']), 12)
        for size in report['sizes'].values():
            self.assertEqual(size['complete_pairs'], 4)
            self.assertNotIn('verify', size['medians'])
            self.assertEqual(size['medians']['open']['pair_count'], 3)

    def test_missing_pair_cannot_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            code, report = self.aggregate(Path(temporary), omit=(3000, 3))
        self.assertEqual(code, 1)
        self.assertEqual(report['status'], 'incomplete')
        self.assertEqual(report['sizes']['3000']['incomplete_pairs'], 1)

    def test_incomplete_pair_cannot_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            code, report = self.aggregate(Path(temporary), incomplete=(1500, 1))
        self.assertEqual(code, 1)
        self.assertEqual(report['status'], 'incomplete')
        self.assertEqual(report['sizes']['1500']['incomplete_pairs'], 1)


class RuntimeDependencyBoundaryTests(unittest.TestCase):
    def binary(self, root):
        path = root / 'synthetic-elf'
        path.write_bytes(b'\x7fELF\x02\x01' + b'\x00' * 12 + b'\x3e\x00')
        return path

    def test_ldd_version_headings_are_not_library_paths(self):
        ldd = ('linux-vdso.so.1 (0x00007fff00000000)\n'
               '\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x00007fff00000001)\n'
               '\t/lib64/ld-linux-x86-64.so.2 (0x00007fff00000002)\n\n'
               'Version information:\n/tmp/synthetic-elf:\n'
               '\tlibc.so.6 (GLIBC_2.34) => /lib/x86_64-linux-gnu/libc.so.6\n')
        with tempfile.TemporaryDirectory() as temporary:
            binary = self.binary(Path(temporary))
            with mock.patch.object(artifacts, 'command', side_effect=lambda *args: ldd if args[0] == 'ldd' else 'portable ELF\n'), \
                    mock.patch.object(artifacts, 'digest', return_value='e' * 64) as digest:
                result = artifacts.elf_identity(binary)
        expected = ['/lib/x86_64-linux-gnu/libc.so.6', '/lib64/ld-linux-x86-64.so.2']
        self.assertEqual(list(result['resolved_libraries']), expected)
        self.assertEqual([call.args[0] for call in digest.call_args_list], expected)

    def test_manifest_hash_mismatch_is_rejected_before_runtime_checks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = root / 'bundle'
            bundle.mkdir()
            (bundle / 'manifest.json').write_text('{}\n')
            audit = root / 'audit'
            with mock.patch.dict(os.environ, {'ZXS_EXPECTED_MANIFEST_SHA256': '0' * 64}, clear=True), \
                    mock.patch.object(artifacts, 'host_identity') as host, \
                    mock.patch.object(artifacts, 'command') as command:
                with self.assertRaisesRegex(RuntimeError, 'manifest hash differs'):
                    artifacts.verify_bundle(bundle, {}, audit)
            host.assert_not_called()
            command.assert_not_called()
            record = json.loads((audit / 'identity-verification.json').read_text())
            self.assertEqual(record['status'], 'failed')
            self.assertEqual(record['manifest_sha256'], artifacts.digest(bundle / 'manifest.json'))
            self.assertIn('manifest hash differs', record['error'])

    def test_unresolved_runtime_dependency_is_rejected(self):
        for missing in ('libbad.so => not found\n', 'GLIBC_2.99 => not found\n'):
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as temporary:
                binary = self.binary(Path(temporary))
                with mock.patch.object(artifacts, 'command', side_effect=lambda *args: missing if args[0] == 'ldd' else 'portable ELF\n'):
                    with self.assertRaisesRegex(RuntimeError, 'unresolved runtime dependency'):
                        artifacts.elf_identity(binary)


if __name__ == '__main__':
    unittest.main()
