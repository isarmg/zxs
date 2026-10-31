"""New product binding rejects stale identities; never starts a benchmark."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from test_screen import pair

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/local-rpc-acceptance'))
import candidate_220 as CANDIDATE


class CandidateBindingTests(unittest.TestCase):
    def setUp(self):
        self.env = patch.dict(os.environ, dict(RPC_SOURCE_SHA='c'*40, GITHUB_SHA='d'*40), clear=True)
        self.env.start()
        self.old = CANDIDATE.screen.PINS
        self.pins = CANDIDATE.configure('a'*40, 'b'*40)

    def tearDown(self):
        CANDIDATE.screen.PINS = self.old
        self.env.stop()

    def bundle(self, root):
        binaries = {}
        for label in self.pins:
            for kind in ('seed', 'server'):
                name = f'{label}-{kind}'
                (root / name).write_bytes(name.encode())
                binaries[name] = CANDIDATE.screen.sha(root / name)
        manifest = dict(controller=dict(source_pins=self.pins, sha='d'*40,
                        normal_source_sha='c'*40, normal_source_tree='e'*40),
                        source={label: dict(product_sha=sha, base_tree=tree, production_source_changes=False)
                                for label, (sha, tree) in self.pins.items()}, binaries=binaries)
        CANDIDATE.screen.write(root / 'manifest.json', manifest)
        os.environ['RPC_MANIFEST_SHA256'] = CANDIDATE.screen.sha(root / 'manifest.json')
        return manifest

    def git_identity(self, root, *args):
        self.assertEqual(args[:1], ('rev-parse',))
        return 'c'*40 if args[1] == 'HEAD' else 'e'*40

    def test_exact_source_tree_manifest_and_all_binary_hashes(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(CANDIDATE.screen, 'git', self.git_identity):
            root = Path(tmp)
            original = self.bundle(root)
            self.assertEqual(CANDIDATE.checked_manifest(root, self.pins), original)
            for mutate in [lambda m: m['source']['candidate'].update(product_sha='f'*40),
                           lambda m: m['source']['candidate'].update(base_tree='f'*40),
                           lambda m: m['controller'].update(normal_source_sha='f'*40),
                           lambda m: m['controller']['source_pins'].update(candidate=['f'*40, 'b'*40]),
                           lambda m: m['source']['candidate'].update(production_source_changes=True)]:
                changed = copy.deepcopy(original)
                mutate(changed)
                CANDIDATE.screen.write(root / 'manifest.json', changed)
                os.environ['RPC_MANIFEST_SHA256'] = CANDIDATE.screen.sha(root / 'manifest.json')
                with self.assertRaises(AssertionError): CANDIDATE.checked_manifest(root, self.pins)
            self.bundle(root)
            os.environ['RPC_MANIFEST_SHA256'] = '0'*64
            with self.assertRaises(AssertionError): CANDIDATE.checked_manifest(root, self.pins)
            self.bundle(root)
            (root / 'candidate-seed').write_bytes(b'changed')
            with self.assertRaises(AssertionError): CANDIDATE.checked_manifest(root, self.pins)

    def test_no_abbreviated_sha_wrong_tree_or_old_candidate_reuse(self):
        for sha, tree in [('abc', 'b'*40), ('a'*40, 'bad'), (CANDIDATE.BASELINE[0], 'b'*40)]:
            with self.assertRaises(AssertionError): CANDIDATE.configure(sha, tree)
        os.environ['RPC_REUSE_BUILD'] = '41166634669'
        with self.assertRaises(AssertionError): CANDIDATE.configure('a'*40, 'b'*40)

    def test_plan_budget_tampering_and_wrong_plan_hash_fail(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(CANDIDATE.screen, 'git', self.git_identity):
            root = Path(tmp)
            self.bundle(root)
            plan = root / 'plan.json'
            CANDIDATE.select(root, plan, self.pins)
            os.environ['RPC_PLAN_SHA256'] = CANDIDATE.screen.sha(plan)
            original = CANDIDATE.checked_plan(plan, self.pins)
            for key, value in [('phase_seconds', 1201), ('cpus', 1), ('pair_seconds', 3600),
                               ('address_space_bytes', 16*CANDIDATE.screen.GIB),
                               ('manifest_sha256', '0'*64), ('continuous_extension_support', 'PASS')]:
                changed = copy.deepcopy(original); changed[key] = value
                CANDIDATE.screen.write(plan, changed)
                os.environ['RPC_PLAN_SHA256'] = CANDIDATE.screen.sha(plan)
                with self.assertRaises(AssertionError): CANDIDATE.checked_plan(plan, self.pins)
            CANDIDATE.screen.write(plan, original)
            os.environ['RPC_PLAN_SHA256'] = '0'*64
            with self.assertRaises(AssertionError): CANDIDATE.checked_plan(plan, self.pins)

    def original_pair(self):
        data = pair()
        data.update(profile='original-220', controller_sha='d'*40, normal_source_sha='c'*40, reused_build=None)
        for arm in data['runs']:
            del arm['phases']['tails']; del arm['phases']['tails-verify']
        return data

    def test_original_220_order_oracles_limits_and_failure_remain(self):
        data = self.original_pair()
        plan = dict(manifest_sha256='sealed', controller_sha='d'*40, normal_source_sha='c'*40)
        checks = CANDIDATE.compare(data, 0, plan)['checks']
        self.assertEqual(len(checks), 8)
        self.assertEqual(checks['open_vmhwm']['ratio_limit'], .8)
        self.assertEqual(checks['startup']['ratio_limit'], 1.1)
        self.assertEqual(checks['save_p95']['limit_ns'], 50_000_000)
        for mutate in [lambda d: d.update(manifest_sha256='wrong'), lambda d: d.update(pair=1),
                       lambda d: d.update(order=['candidate', 'baseline']),
                       lambda d: d['runs'][0]['phases']['rpc-verify'].update(saved_receipts=19)]:
            changed = copy.deepcopy(data); mutate(changed)
            with self.assertRaises(AssertionError): CANDIDATE.compare(changed, 0, plan)
        data['runs'][1]['phases']['requests']['open_ns'] = 111
        self.assertFalse(CANDIDATE.compare(data, 0, plan)['checks']['startup']['passed'])

    def test_tracked_and_untracked_benches_have_exact_before_after_identity(self):
        for tracked in (False, True):
            with self.subTest(tracked=tracked), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                def git(*args):
                    return subprocess.check_output(['git', '-C', str(root), *args], stderr=subprocess.DEVNULL)
                git('init')
                for name in CANDIDATE.screen.TRACKED + (CANDIDATE.screen.ADDED if tracked else []):
                    p = root / name; p.parent.mkdir(parents=True, exist_ok=True); p.write_text('original\n')
                git('add', '.')
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'fixture')
                for name in CANDIDATE.screen.TRACKED:
                    with (root / name).open('a') as out: out.write('wire\n')
                for name in CANDIDATE.screen.ADDED:
                    p = root / name; p.parent.mkdir(parents=True, exist_ok=True)
                    if not tracked: p.write_text('adapter\n')
                identity = CANDIDATE.screen.identity(root)
                for name in CANDIDATE.screen.ADDED:
                    self.assertEqual(identity['overlay'][name]['before_sha256'] is not None, tracked)
                if tracked:
                    (root / CANDIDATE.screen.ADDED[0]).write_text('tampered\n')
                    with self.assertRaises(AssertionError): CANDIDATE.screen.identity(root)
                else:
                    (root / 'unexpected').write_text('not an adapter')
                    with self.assertRaises(AssertionError): CANDIDATE.screen.identity(root)

    def test_summary_scopes_new_product_and_retains_old_capacity_failure(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(CANDIDATE.screen, 'git', self.git_identity):
            root = Path(tmp)
            self.bundle(root)
            plan = root / 'plan.json'
            CANDIDATE.select(root, plan, self.pins)
            os.environ['RPC_PLAN_SHA256'] = CANDIDATE.screen.sha(plan)
            for number in (0, 1):
                data = self.original_pair()
                data['pair'] = number
                data['manifest_sha256'] = os.environ['RPC_MANIFEST_SHA256']
                if number:
                    data['order'].reverse(); data['runs'].reverse()
                directory = root / f'rpc-220-original-{number}'
                directory.mkdir()
                CANDIDATE.screen.write(directory / 'pair.json', data)
            output = root / 'summary.json'
            with patch('builtins.print'):
                CANDIDATE.summary(root, plan, output, self.pins)
            report = json.loads(output.read_text())
            self.assertEqual(report['original_220_gate'], 'PASS')
            self.assertEqual(report['continuous_extension_support'], 'NOT_EVALUATED_BY_ORIGINAL_220')
            self.assertEqual(report['full_extension_acceptance'], 'NOT_EVALUATED_BY_ORIGINAL_220')
            self.assertIsNone(report['actual_continuous_run'])
            self.assertTrue(report['old_continuous']['status'].startswith('FAIL_DEFAULT_BUDGET retained'))
            self.assertIn('old 64-byte reservation', report['old_continuous']['status'])
            self.assertEqual(report['old_original_220']['status'], 'FAIL retained')

            failing = root / 'rpc-220-original-0' / 'pair.json'
            data = json.loads(failing.read_text())
            data['runs'][1]['phases']['requests']['open_ns'] = data['runs'][0]['phases']['requests']['open_ns'] * 2
            CANDIDATE.screen.write(failing, data)
            with patch('builtins.print'):
                CANDIDATE.summary(root, plan, output, self.pins)
            failed = json.loads(output.read_text())
            self.assertEqual(failed['delivery_gate'], 'PASS')
            self.assertFalse(failed['pairs'][0]['checks']['startup']['blocking'])
            self.assertEqual(failed['original_220_gate'], 'FAIL')
            self.assertEqual(failed['full_extension_acceptance'], 'NOT_EVALUATED_BY_ORIGINAL_220')
