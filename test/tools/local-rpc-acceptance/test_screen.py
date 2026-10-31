"""Finite controller failure handling; no benchmark executes in these tests."""
import copy
import importlib.util
import json
import contextlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / 'tools/local-rpc-acceptance/screen.py'
SPEC = importlib.util.spec_from_file_location('final_rpc_acceptance', SCRIPT)
SCREEN = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCREEN)


def arm(label, commits):
    return dict(label=label, status='completed', inventory={}, phases={
        'build': {}, 'edit': {},
        'verify': dict(verified_current_files=500, verified_operations=commits,
                       verified_commit_namespaces=commits, verified_bodies=500+commits-2),
        'rpc-verify': dict(sequence=commits+20, current_files=500, saved_receipts=20,
                           saved_commit_namespaces=20, saved_historical_bodies=20),
        'requests': dict(schema='final-local-rpc-v1', commits_before=commits,
                         verified_reads=1001, verified_lists=1000, verified_receipts=20,
                         read_ns=[1]*1000, read_open_ns=[1]*1000, list_ns=[1]*1000,
                         save_ns=[1]*20, save_batch_ns=20, open_ns=100,
                         open_memory=dict(vmhwm_kib=100 if label == 'baseline' else 80), recovery_warnings={}),
        'tails': dict(schema='final-local-rpc-tails-v1', commits_before=commits+20, samples_per_metric=40,
                      published_generations=120, body_bytes=8*1024*1024, append_bytes=64*1024,
                      flush_ns=[1]*40, close_ns=[1]*40, write_through_ns=[1]*40),
        'tails-verify': dict(sequence=commits+140, saved_receipts=120, saved_historical_bodies=120,
                            current_tail_files=80, independent_full_copies=1)})


def pair(commits=220, number=0):
    order = ['baseline', 'candidate'] if number == 0 else ['candidate', 'baseline']
    return dict(status='completed', commits=commits, pair=number, order=order, manifest_sha256='sealed',
                host=dict(child_cpus=[0, 1]), runs=[arm(label, commits) for label in order])


class ControllerTests(unittest.TestCase):
    def test_nearest_rank_small_sample_p99_is_max(self):
        self.assertEqual(SCREEN.quantile(list(range(40)), .99), 39)
        self.assertEqual(SCREEN.quantile(list(range(20)), .95), 18)

    def test_original_limits_and_tail_limits(self):
        result = SCREEN.compare_pair(pair(), 220, 0, 'sealed')['checks']
        self.assertEqual(result['read_p95']['limit_ns'], 2_000_000)
        self.assertEqual(result['read_p99']['limit_ns'], 5_000_000)
        self.assertEqual(result['save_batch']['limit_ns'], 1_000_000_000)
        for name in ('save_p95', 'flush_p95', 'close_p95', 'write_through_p95'):
            self.assertEqual(result[name]['limit_ns'], 50_000_000)
        self.assertEqual(result['open_vmhwm']['ratio_limit'], .8)
        self.assertEqual(result['startup']['ratio_limit'], 1.1)
        self.assertTrue(all(check['passed'] for check in result.values()))

    def test_missing_samples_and_oracles_fail(self):
        data = pair()
        mutations = [lambda d: d.update(status='incomplete'),
                     lambda d: d.update(manifest_sha256='different'),
                     lambda d: d.update(order=['candidate', 'baseline']),
                     lambda d: d['runs'][0]['phases']['requests']['save_ns'].pop(),
                     lambda d: d['runs'][0]['phases']['tails']['flush_ns'].pop(),
                     lambda d: d['runs'][0]['phases']['tails-verify'].update(saved_receipts=119),
                     lambda d: d['runs'][0]['phases']['requests']['read_ns'].__setitem__(0, -1)]
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                changed = copy.deepcopy(data)
                mutate(changed)
                with self.assertRaises(AssertionError):
                    SCREEN.compare_pair(changed, 220, 0, 'sealed')

    def test_single_pair_regression_remains_failed(self):
        data = pair()
        data['runs'][1]['phases']['requests']['save_ns'] = [50_000_002]*20
        checks = SCREEN.compare_pair(data, 220, 0, 'sealed')['checks']
        self.assertFalse(checks['save_p95']['passed'])
        self.assertEqual(checks['save_p95']['delta_ns'], 50_000_001)

    def test_summary_fails_process_and_preserves_historical_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for commits in (22, 220):
                for number in (0, 1):
                    data = pair(commits, number)
                    if (commits, number) == (220, 1):
                        data['runs'][0]['phases']['tails']['close_ns'] = [50_000_002]*40
                    directory = root / f'rpc-{commits}-{number}'
                    directory.mkdir()
                    (directory/'pair.json').write_text(json.dumps(data))
            with patch.dict(os.environ, RPC_MANIFEST_SHA256='sealed'):
                with self.assertRaises(SystemExit) as failed:
                    with contextlib.redirect_stdout(io.StringIO()):
                        SCREEN.summary(root, root/'summary.json')
            self.assertEqual(failed.exception.code, 1)
            report = json.loads((root/'summary.json').read_text())
            self.assertEqual(report['status'], 'completed')
            self.assertEqual(report['prospective_screen'], 'FAIL')
            self.assertEqual(report['old_parity_gate'], 'FAILED unchanged')

    def test_no_partial_summary_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp)/'summary.json'
            with self.assertRaises(FileNotFoundError):
                with contextlib.redirect_stdout(io.StringIO()):
                    SCREEN.summary(Path(tmp), output)
            self.assertEqual(json.loads(output.read_text())['prospective_screen'], 'NOT EVALUATED')

    def test_python_optimization_cannot_remove_assertions(self):
        result = subprocess.run([sys.executable, '-O', str(SCRIPT)], capture_output=True, text=True, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('optimized Python', result.stderr)

    def test_workload_cannot_execute_outside_actions(self):
        with patch.dict(os.environ, GITHUB_ACTIONS='false'):
            with self.assertRaises(AssertionError):
                SCREEN.run(Path('missing'), Path('never-created'), 22, 0)

    def test_pins_exclude_rejected_architecture(self):
        self.assertEqual(SCREEN.PINS['candidate'][0], 'd3eefd289e53a2803703870fe4250d08cadff9b5')
        self.assertEqual(SCREEN.PINS['baseline'][0], 'e2f33d9882cf7263412d9370b6a703e0b216357d')


if __name__ == '__main__':
    unittest.main()
