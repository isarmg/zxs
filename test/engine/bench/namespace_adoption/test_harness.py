#!/usr/bin/env python3
"""Tool-boundary regressions using synthetic child output; no product is executed."""
import contextlib
import copy
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
SYNTHETIC_INPUTS = {label: {'adapter_path': f'test/engine/bench/namespace_adoption/{label}.rs'}
                    for label in ('baseline', 'candidate')}
ENVIRONMENT = dict(GITHUB_RUN_ID='1234', GITHUB_RUN_ATTEMPT='1', GITHUB_SHA='a' * 40,
                   ZXS_EXPECTED_MANIFEST_SHA256='b' * 64, ZXS_BUILD_RESULT='success',
                   GITHUB_JOB='paired', RUNNER_NAME='synthetic-runner')


class RunnerBoundaryTests(unittest.TestCase):
    def phase_with_output(self, root, phase, rows, traced=False, observer='plain', stderr_payload='', returncode=0):
        evidence = root / 'evidence'
        evidence.mkdir()
        payload = ''.join(json.dumps(row) + '\n' for row in rows).encode()

        def launch(command, stdout, stderr, **options):
            stdout.write(payload)
            stdout.flush()
            stderr.write(stderr_payload.encode())
            stderr.flush()
            return SimpleNamespace(pid=123, poll=lambda: returncode, wait=lambda: returncode)

        with mock.patch.object(runner.subprocess, 'Popen', side_effect=launch):
            return runner.run_phase(Path('/synthetic/binary'), phase, 500, root,
                                    evidence, traced, runner.time.time() + 60, observer)

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
                               sizes=[500], pair_index=0, mode='final', trace_observer='plain', observer_probe=None)

        def verify(*unused, **options):
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

            with mock.patch.object(runner, 'load_inputs', return_value=SYNTHETIC_INPUTS), \
                    mock.patch.object(runner, 'parse_arguments', return_value=args), \
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
            with mock.patch.object(runner, 'load_inputs', return_value=SYNTHETIC_INPUTS), \
                    mock.patch.object(runner, 'parse_arguments', return_value=args), \
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
                                   deadline_unix=runner.time.time() + 60, trace_observer='plain')
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

    def test_trace_modes_keep_identical_nine_syscall_collection(self):
        target = ['/synthetic/binary', 'verify', '500', '/synthetic/pool']
        for observer in ('plain', 'seccomp-bpf'):
            with self.subTest(observer=observer):
                command = runner.trace_command(target, Path('/evidence'), 'verify', observer)
                self.assertEqual(command[-4:], target)
                self.assertIn('trace=' + ','.join(runner.TRACE_SYSCALLS), command)
                self.assertEqual(len(runner.TRACE_SYSCALLS), 9)
                self.assertIn('-f', command)
                self.assertIn('-c', command)
                self.assertEqual('--seccomp-bpf' in command, observer == 'seccomp-bpf')
                self.assertEqual('-d' in command, observer == 'seccomp-bpf')

    def test_bpf_requires_real_enabled_and_wait_event_evidence(self):
        enabled = 'strace: seccomp filter enabled\n'
        event = 'strace: [wait(0x07057f) = 4321] WIFSTOPPED,sig=SIGTRAP,EVENT_SECCOMP (7)\n'
        for stderr, expected in ((enabled + event, 'completed'), (enabled, 'incomplete'),
                                 ('EVENT_SECCOMP (7)\n', 'incomplete'), ('', 'incomplete')):
            with self.subTest(stderr=stderr), tempfile.TemporaryDirectory() as temporary:
                result = self.phase_with_output(Path(temporary), 'verify', [dict(phase='verify', files=500)],
                                                traced=True, observer='seccomp-bpf', stderr_payload=stderr)
                self.assertEqual(result['status'], expected)
                self.assertEqual(result['measurements'][0]['phase'], 'verify')
                if expected == 'completed':
                    self.assertEqual(result['observer']['effective_mode'], 'seccomp-bpf')
                    self.assertEqual(result['observer']['seccomp_event_pids'], [4321])
                else:
                    self.assertEqual(result['reason'], 'trace_observer_activation_unproven')

    def test_bpf_fallback_and_conflicting_evidence_are_incomplete(self):
        fallback = 'strace: seccomp filter is requested but unavailable\nstrace: seccomp filter disabled\n'
        positive = ('strace: seccomp filter enabled\n'
                    'strace: [wait(0x07057f) = 4321] WIFSTOPPED,sig=SIGTRAP,EVENT_SECCOMP (7)\n')
        for diagnostic, activation in ((fallback, 'fallback_to_plain'),
                                       (positive + fallback, 'unproven_or_conflicting')):
            with self.subTest(activation=activation), tempfile.TemporaryDirectory() as temporary:
                result = self.phase_with_output(Path(temporary), 'open', [dict(phase='open', files=500)],
                                                traced=True, observer='seccomp-bpf', stderr_payload=diagnostic)
                self.assertEqual(result['status'], 'incomplete')
                self.assertEqual(result['observer']['activation_status'], activation)
                self.assertTrue(result['observer']['fallback_diagnostics'])

    def test_bpf_fatal_initialization_is_a_real_process_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            result = self.phase_with_output(Path(temporary), 'open', [], traced=True,
                                            observer='seccomp-bpf', returncode=1,
                                            stderr_payload='strace: prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER): Permission denied\n')
        self.assertEqual((result['status'], result['reason']), ('failed', 'benchmark_process_failed'))
        self.assertEqual(result['observer']['fallback_diagnostics'], [])

    def test_observer_validation_is_confined_to_500_file_trace_pair(self):
        base = ['run.py', '--mode', 'observer-validation', '--bundle-dir', '/bundle', '--audit-dir', '/audit',
                '--output', '/output', '--pool-parent', '/pools', '--deadline-unix', '1000']
        for options in (['--sizes', '1500', '--pair-index', '3'], ['--sizes', '500', '--pair-index', '0']):
            with self.subTest(options=options), mock.patch.object(sys, 'argv', base + options), \
                    contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as failure:
                    runner.parse_arguments()
                self.assertEqual(failure.exception.code, 2)
        with mock.patch.object(sys, 'argv', base + ['--sizes', '500', '--pair-index', '3', '--trace-observer', 'seccomp-bpf']):
            args = runner.parse_arguments()
        self.assertEqual((args.mode, args.sizes, args.pair_index, args.trace_observer),
                         ('observer-validation', [500], 3, 'seccomp-bpf'))

    def test_ambient_filter_fields_require_explicit_unfiltered_parent(self):
        cases = [('NoNewPrivs:\t0\nSeccomp:\t0\nSeccomp_filters:\t0\n', True),
                 ('NoNewPrivs:\t1\nSeccomp:\t2\nSeccomp_filters:\t1\n', False),
                 ('NoNewPrivs:\t0\nSeccomp:\t0\n', False)]
        with tempfile.TemporaryDirectory() as temporary:
            status = Path(temporary) / 'status'
            for text, expected in cases:
                with self.subTest(text=text):
                    status.write_text(text)
                    observation = runner.ambient_observer_status(status)
                    self.assertEqual(observation['unfiltered'], expected)
                    self.assertEqual(len(observation['raw_fields']), len(text.splitlines()))

    def test_existing_ambient_filter_blocks_bpf_but_does_not_block_plain(self):
        ambient = dict(unfiltered=False, fields=dict(NoNewPrivs=0, Seccomp=2, Seccomp_filters=1))
        for observer in ('plain', 'seccomp-bpf'):
            with self.subTest(observer=observer), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                args = SimpleNamespace(output=root / 'results', pool_parent=root / 'pools',
                                       deadline_unix=runner.time.time() + 60, trace_observer=observer, observer_probe=None)
                args.output.mkdir()
                args.pool_parent.mkdir()
                run = runner.planned_runs([500], [3], observer)[0]
                with mock.patch.object(runner, 'ambient_observer_status', return_value=ambient), \
                        mock.patch.object(runner.shutil, 'disk_usage', return_value=SimpleNamespace(free=100 * runner.GIB)), \
                        mock.patch.object(runner, 'run_phase', return_value=dict(status='completed', reason=None)) as phase:
                    runner.measure_implementation(run, Path('/synthetic/binary'), args, mock.Mock())
                if observer == 'seccomp-bpf':
                    phase.assert_not_called()
                    self.assertEqual(run['status'], 'incomplete')
                    self.assertEqual(run['reason'], 'trace_observer_ambient_filter_unsupported')
                    self.assertEqual(run['not_run_phases'], list(runner.PHASES))
                else:
                    self.assertEqual(phase.call_count, 5)
                    self.assertEqual(run['status'], 'completed')
                self.assertEqual(json.loads((args.output / run['name'] / 'observer-preflight.json').read_text()), ambient)



class ProbeAdmissionBoundaryTests(unittest.TestCase):
    def fixture(self, root):
        binary = root / 'strace'
        binary.write_bytes(b'synthetic-strace-executable')
        version = b'strace -- version 6.8\n'
        (root / 'version.stdout').write_bytes(version)
        probe = dict(schema='namespace-observer-probe-v1', status='completed_probe',
                     probe_script_sha256=runner.hashlib.sha256((TOOLS / 'observer_probe.py').read_bytes()).hexdigest(),
                     mode='observer-validation', scope='deterministic-9-syscall-contract-only',
                     github_run_id=ENVIRONMENT['GITHUB_RUN_ID'], github_run_attempt='1',
                     carrier_sha=ENVIRONMENT['GITHUB_SHA'], github_job='paired', runner_name='synthetic-runner',
                     ambient=dict(values=dict(NoNewPrivs=0, Seccomp=0, Seccomp_filters=0)),
                     strace=dict(path=str(binary), sha256=runner.hashlib.sha256(binary.read_bytes()).hexdigest(),
                                 version=version.decode(), version_sha256=runner.hashlib.sha256(version).hexdigest()),
                     probes=[])
        for mode in ('plain', 'seccomp-bpf'):
            workers = []
            for tid, role in enumerate(('parent', 'fork-child', 'thread'), 101):
                values = dict(NoNewPrivs=0, Seccomp=2 if mode == 'seccomp-bpf' else 0,
                              Seccomp_filters=1 if mode == 'seccomp-bpf' else 0)
                workers.append(dict(role=role, tid=tid, proc_self=dict(values=values), proc_thread_self=dict(values=values)))
            command = runner.trace_command(['/synthetic/python', '--worker', '/synthetic/root'], root, mode, mode)
            probe['probes'].append(dict(mode=mode, status='completed',
                counts=dict(syscalls={name: dict(calls=6, errors=3) for name in runner.TRACE_SYSCALLS},
                            total=dict(calls=54, errors=27)),
                process=dict(returncode=0, timed_out=False, command=command),
                observer=dict(requested_mode=mode, effective_mode=mode, syscalls=list(runner.TRACE_SYSCALLS),
                              full_argv=command, activation_status='enabled_and_observed' if mode == 'seccomp-bpf' else 'plain_requested',
                              enabled_marker_count=1, seccomp_event_count=3, seccomp_event_pids=[101, 102, 103],
                              fallback_diagnostics=[]),
                tracees=dict(status='completed', scope='deterministic-syscall-fixture-only', workers=workers)))
        return binary, probe

    def admission(self, root, binary, probe):
        path = root / 'probe.json'
        path.write_text(json.dumps(probe))
        with mock.patch.dict(os.environ, ENVIRONMENT, clear=True), \
                mock.patch.object(runner.shutil, 'which', return_value=str(binary)):
            return runner.verify_observer_probe(path)

    def test_exact_current_job_probe_admits_bpf(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, probe = self.fixture(root)
            admission = self.admission(root, binary, probe)
            self.assertEqual(admission['status'], 'verified')
            self.assertEqual(admission['path'], str(root / 'probe.json'))
            self.assertEqual(admission['sha256'], runner.hashlib.sha256((root / 'probe.json').read_bytes()).hexdigest())

    def test_failed_cross_job_miscount_or_unproven_probe_cannot_admit(self):
        mutations = [lambda p: p.update(status='incomplete'),
                     lambda p: p.update(github_run_id='different-run'),
                     lambda p: p.update(carrier_sha='f' * 40),
                     lambda p: p.update(github_job='another-job'),
                     lambda p: p.update(runner_name='another-runner'),
                     lambda p: p['strace'].update(sha256='0' * 64),
                     lambda p: p.update(probe_script_sha256='0' * 64),
                     lambda p: p['strace'].update(version='changed-version'),
                     lambda p: p['probes'][1]['counts']['syscalls']['fsync'].update(calls=5),
                     lambda p: p['probes'][1]['observer'].update(fallback_diagnostics=['unavailable']),
                     lambda p: p['probes'][1]['observer'].update(seccomp_event_pids=[101]),
                     lambda p: p['probes'][1]['tracees']['workers'][0]['proc_self']['values'].update(Seccomp_filters=2)]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, original = self.fixture(root)
            for index, mutate in enumerate(mutations):
                with self.subTest(index=index):
                    probe = copy.deepcopy(original)
                    mutate(probe)
                    admission = self.admission(root, binary, probe)
                    self.assertEqual(admission['status'], 'incomplete')
                    self.assertIn('reason', admission)

    def test_missing_probe_stops_product_launch_and_preserves_admission(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(output=root / 'results', pool_parent=root / 'pools',
                                   deadline_unix=runner.time.time() + 60, trace_observer='seccomp-bpf', observer_probe=None)
            args.output.mkdir()
            args.pool_parent.mkdir()
            run = runner.planned_runs([500], [3], 'seccomp-bpf')[0]
            with mock.patch.object(runner, 'ambient_observer_status', return_value=dict(unfiltered=True)), \
                    mock.patch.object(runner, 'run_phase') as phase:
                runner.measure_implementation(run, Path('/synthetic/product'), args, mock.Mock())
            phase.assert_not_called()
            self.assertEqual((run['status'], run['reason']), ('incomplete', 'trace_observer_probe_not_verified'))
            record = json.loads((args.output / run['name'] / 'observer-probe-admission.json').read_text())
            self.assertEqual(record['status'], 'incomplete')
            self.assertIn('required', record['reason'])



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
                phase_record = dict(phase=phase, status='completed', returncode=0, measurements=rows)
                if pair == 3:
                    command = runner.trace_command(['/synthetic/binary', phase, str(files), '/synthetic/pool'],
                                                   Path('/synthetic/evidence'), phase, 'plain')
                    phase_record.update(command=command, traced=True, observer=dict(
                        requested_mode='plain', effective_mode='plain', activation_status='plain_requested',
                        syscalls=list(runner.TRACE_SYSCALLS), full_argv=command))
                phases.append(phase_record)
            runs.append(dict(implementation=label, files=files, repetition=pair, traced=pair == 3,
                             binary_sha256=hashes[label], status='completed', phases=phases,
                             trace_observer_requested='plain' if pair == 3 else 'none',
                             observer_preflight=dict(unfiltered=True, fields=dict(NoNewPrivs=0, Seccomp=0, Seccomp_filters=0)),
                             attempted_phases=list(runner.PHASES), not_run_phases=[]))
        return dict(files=files, sizes=[files], pair_index=pair, order=order,
                    validation_mode='final', trace_observer='plain',
                    github_run_id=ENVIRONMENT['GITHUB_RUN_ID'], github_run_attempt='1',
                    carrier_sha=ENVIRONMENT['GITHUB_SHA'],
                    artifact_manifest_sha256=ENVIRONMENT['ZXS_EXPECTED_MANIFEST_SHA256'],
                    artifact_identity=dict(status='verified', mode='final', manifest_sha256=ENVIRONMENT['ZXS_EXPECTED_MANIFEST_SHA256']),
                    binary_sha256=hashes, status='completed_measurements', runs=runs)

    def aggregate(self, root, omit=None, incomplete=None, missing_trace=False, failed_audit=False):
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
                if failed_audit and (files, pair) == (500, 0):
                    summary['artifact_identity']['status'] = 'failed'
                (directory / 'summary.json').write_text(json.dumps(summary))
                if pair == 3:
                    for label in ('baseline', 'candidate'):
                        run_directory = directory / f'{files}-{pair}-{label}'
                        run_directory.mkdir()
                        for phase in runner.PHASES:
                            if missing_trace and (files, label, phase) == (500, 'baseline', 'verify'):
                                continue
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

    def test_completed_trace_pair_requires_raw_trace_for_every_phase(self):
        with tempfile.TemporaryDirectory() as temporary:
            code, report = self.aggregate(Path(temporary), missing_trace=True)
        self.assertEqual(code, 1)
        self.assertEqual(report['sizes']['500']['failed_pairs'], 1)

    def test_failed_artifact_audit_cannot_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            code, report = self.aggregate(Path(temporary), failed_audit=True)
        self.assertEqual(code, 1)
        self.assertEqual(report['sizes']['500']['failed_pairs'], 1)


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

    def test_pending_final_candidate_never_falls_back_to_observer_candidate(self):
        config = json.loads((TOOLS / 'inputs.json').read_text())
        config['final_candidate'] = None
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'inputs.json'
            path.write_text(json.dumps(config))
            with self.assertRaisesRegex(RuntimeError, 'candidate SHA/tree/adapter identity is pending'):
                artifacts.load_inputs('final', path)
            selected = artifacts.load_inputs('observer-validation', path)
        self.assertEqual(selected['candidate']['sha'], config['observer_validation_candidate']['sha'])
        self.assertEqual(selected['candidate']['adapter_path'], config['observer_validation_candidate']['adapter_path'])

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
                    artifacts.verify_bundle(bundle, {}, audit, mode='observer-validation')
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
