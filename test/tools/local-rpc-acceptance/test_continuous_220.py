"""Separate engineering-control and capacity scopes; no storage workload runs."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from test_screen import pair
import test_candidate_220 as candidate_tests
import continuous_220 as CONTINUOUS


class ContinuousTests(unittest.TestCase):
    bundle = candidate_tests.CandidateBindingTests.bundle
    git_identity = candidate_tests.CandidateBindingTests.git_identity

    def setUp(self):
        self.env = patch.dict(os.environ, dict(RPC_SOURCE_SHA='c'*40, GITHUB_SHA='d'*40), clear=True)
        self.env.start()
        self.old = CONTINUOUS.screen.PINS
        self.pins = CONTINUOUS.configure('a'*40, 'b'*40, 'f'*40, CONTINUOUS.CONTROL_TREE)

    def tearDown(self):
        CONTINUOUS.screen.PINS = self.old
        self.env.stop()

    def seal(self, root):
        manifest = self.bundle(root)
        manifest['controller']['engineering_control'] = CONTINUOUS.provenance(self.pins)
        CONTINUOUS.screen.write(root / 'manifest.json', manifest)
        os.environ['RPC_MANIFEST_SHA256'] = CONTINUOUS.screen.sha(root / 'manifest.json')
        return manifest

    def data(self, number=0):
        data = pair(number=number)
        data.update(profile='continuous-220', controller_sha='d'*40, normal_source_sha='c'*40, reused_build=None)
        for arm in data['runs']:
            capacity = {scope: dict(records=dict(used='0'), cold_records='359', metadata_bytes=dict(used='123456', limit=str(limit)))
                        for scope, limit in [('principal', 64*1024*1024), ('global', 128*1024*1024)]}
            arm['phases']['tails-verify'].update(retained_seed_receipts=220, retained_seed_bodies=718,
                retained_rpc_receipts=20, retained_rpc_bodies=20, retained_total_operations=360,
                retained_total_events=360, retained_workspaces=359, principal='bench', capacity=capacity)
        return data

    def test_control_tree_parent_and_single_delta_are_not_arbitrary(self):
        with self.assertRaises(AssertionError): CONTINUOUS.configure('a'*40, 'b'*40, 'f'*40, 'e'*40)
        with self.assertRaises(AssertionError): CONTINUOUS.configure('a'*40, 'b'*40, CONTINUOUS.original.BASELINE[0], CONTINUOUS.CONTROL_TREE)
        with patch.object(CONTINUOUS.screen, 'git', return_value='not-the-original-parent'), patch.object(CONTINUOUS.screen, 'prepare') as prepare:
            with self.assertRaises(AssertionError): CONTINUOUS.prepare(Path('/unused'), Path('/unused-evidence'), self.pins)
            prepare.assert_not_called()
        with patch.object(CONTINUOUS.screen, 'git', side_effect=[CONTINUOUS.original.BASELINE[0], CONTINUOUS.CONTROL_DELTA+'\nother.rs']), patch.object(CONTINUOUS.screen, 'prepare') as prepare:
            with self.assertRaises(AssertionError): CONTINUOUS.prepare(Path('/unused'), Path('/unused-evidence'), self.pins)
            prepare.assert_not_called()

    def test_seal_provenance_plan_identity_and_budget_are_required(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(CONTINUOUS.screen, 'git', self.git_identity):
            root = Path(tmp); manifest = self.seal(root)
            CONTINUOUS.checked_manifest(root, self.pins)
            manifest['controller']['engineering_control']['only_delta'] = 'other.rs'
            CONTINUOUS.screen.write(root/'manifest.json', manifest)
            os.environ['RPC_MANIFEST_SHA256'] = CONTINUOUS.screen.sha(root/'manifest.json')
            with self.assertRaises(AssertionError): CONTINUOUS.checked_manifest(root, self.pins)
            self.seal(root)
            path = root/'plan.json'; CONTINUOUS.select(root, path, self.pins)
            os.environ['RPC_PLAN_SHA256'] = CONTINUOUS.screen.sha(path)
            original = CONTINUOUS.checked_plan(path, self.pins)
            for key, value in [('principal', 'other'), ('final_sequence', 240), ('phase_seconds', 1201),
                               ('pair_seconds', 3600), ('principal_metadata_limit', 128*1024*1024),
                               ('manifest_sha256', '0'*64)]:
                changed = copy.deepcopy(original); changed[key] = value
                CONTINUOUS.screen.write(path, changed); os.environ['RPC_PLAN_SHA256'] = CONTINUOUS.screen.sha(path)
                with self.assertRaises(AssertionError): CONTINUOUS.checked_plan(path, self.pins)

    def test_seven_phases_all_retained_history_principal_and_no_budget_reset(self):
        plan = dict(manifest_sha256='sealed', controller_sha='d'*40, normal_source_sha='c'*40)
        self.assertEqual(len(CONTINUOUS.compare(self.data(), 0, plan)['checks']), 11)
        for mutate in [lambda d: d['runs'][0]['phases'].pop('tails'),
                       lambda d: d['runs'][0]['phases']['tails']['flush_ns'].pop(),
                       lambda d: d['runs'][0]['phases']['tails-verify'].update(retained_seed_receipts=219),
                       lambda d: d['runs'][0]['phases']['tails-verify'].update(retained_rpc_bodies=19),
                       lambda d: d['runs'][0]['phases']['tails-verify'].update(principal='other'),
                       lambda d: d['runs'][0]['phases']['tails-verify']['capacity']['principal']['metadata_bytes'].update(used='0'),
                       lambda d: d['runs'][0]['phases']['tails-verify']['capacity']['principal']['metadata_bytes'].update(limit=str(128*1024*1024)),
                       lambda d: d['runs'][0]['phases']['tails-verify']['capacity']['global'].update(cold_records='358')]:
            data = self.data(); mutate(data)
            with self.assertRaises((AssertionError, KeyError)): CONTINUOUS.compare(data, 0, plan)

    def test_full_stage_runtime_is_bound_to_original_45_minute_window(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(CONTINUOUS.screen, 'git', self.git_identity):
            root = Path(tmp); self.seal(root)
            path = root/'plan.json'; CONTINUOUS.select(root, path, self.pins)
            os.environ['RPC_PLAN_SHA256'] = CONTINUOUS.screen.sha(path)
            plan = CONTINUOUS.checked_plan(path, self.pins)
            with patch.object(CONTINUOUS.screen, 'run') as run:
                CONTINUOUS.run(root, root/'out', 0, path, self.pins)
                run.assert_called_once_with(root, root/'out', 220, 0, continuous_only=True, execution_deadline=plan['deadline_unix'])
            with patch.object(CONTINUOUS.time, 'time', return_value=plan['deadline_unix']), patch.object(CONTINUOUS.screen, 'run') as run:
                with self.assertRaises(AssertionError): CONTINUOUS.run(root, root/'out', 0, path, self.pins)
                run.assert_not_called()

    def test_capacity_and_engineering_success_never_relabel_original_failures(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(CONTINUOUS.screen, 'git', self.git_identity):
            root = Path(tmp); self.seal(root)
            path = root/'plan.json'; CONTINUOUS.select(root, path, self.pins)
            os.environ['RPC_PLAN_SHA256'] = CONTINUOUS.screen.sha(path)
            for number in (0, 1):
                directory = root/f'rpc-220-continuous-{number}'; directory.mkdir()
                data = self.data(number); data['manifest_sha256'] = os.environ['RPC_MANIFEST_SHA256']
                CONTINUOUS.screen.write(directory/'pair.json', data)
            output = root/'summary.json'
            with patch('builtins.print'): CONTINUOUS.summary(root, path, output, self.pins)
            report = json.loads(output.read_text())
            self.assertEqual(report['candidate_continuous_capacity'], 'PASS')
            self.assertEqual(report['engineering_eleven_gate'], 'PASS')
            self.assertTrue(report['original_baseline_gate'].startswith('NOT EVALUATED HERE'))
            self.assertEqual(report['old_original_220']['status'], 'FAIL retained')
            self.assertTrue(report['old_continuous']['status'].startswith('FAIL_DEFAULT_BUDGET retained'))

    def test_completed_candidate_survives_failed_control_without_claiming_pair_pass(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(CONTINUOUS.screen, 'git', self.git_identity):
            root = Path(tmp); self.seal(root)
            path = root/'plan.json'; CONTINUOUS.select(root, path, self.pins)
            os.environ['RPC_PLAN_SHA256'] = CONTINUOUS.screen.sha(path)
            directory = root/'rpc-220-continuous-1'; directory.mkdir()
            data = self.data(1); data['manifest_sha256'] = os.environ['RPC_MANIFEST_SHA256']
            data['status'] = 'incomplete'; data['runs'][1]['status'] = 'incomplete'
            data['runs'][1]['phases'].pop('tails-verify')
            CONTINUOUS.screen.write(directory/'pair.json', data)
            output = root/'summary.json'
            with patch('builtins.print'), self.assertRaises(SystemExit):
                CONTINUOUS.summary(root, path, output, self.pins)
            report = json.loads(output.read_text())
            self.assertEqual(report['candidate_samples_passed'], 1)
            self.assertEqual(report['candidate_samples_required'], 2)
            self.assertEqual(report['candidate_continuous_capacity'], 'PARTIAL: 1/2 verified')
            self.assertEqual(report['engineering_eleven_gate'], 'NOT EVALUATED')
            self.assertEqual(report['engineering_tail_latency_gate'], 'NOT EVALUATED')
            data['manifest_sha256'] = '0'*64
            CONTINUOUS.screen.write(directory/'pair.json', data)
            with patch('builtins.print'), self.assertRaises(SystemExit):
                CONTINUOUS.summary(root, path, output, self.pins)
            self.assertEqual(json.loads(output.read_text())['candidate_samples_passed'], 0)
