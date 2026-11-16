"""Select the actual Published-only original FILE no-clobber boundary."""
import json
from pathlib import Path
import unittest

from test_targeted_ci import CI, assert_path_group_extension, origin_selections

ROOT = Path(__file__).resolve().parents[2]
GROUP = 'backup-published-original-files'
EXACT = (
    'engine::backup_preparation::native_control::databases::scoped::cutover::'
    'registration::serving_activation::original_objects::tests::'
    'published_original_files_keep_no_clobber_readback_and_same_decision_retry'
)
PATHS = [
    "src/engine/src/container_storage/ordinary_preparation/backup/originals.rs",
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/serving_activation/original_objects.rs",
    "test/engine/unit/backup_original_published_tests.rs",
    "test/ci/test_backup_published_original_ci.py",
]


class PublishedOriginalFileSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_real_kernel_and_fixture_retain_all_existing_selections(self):
        for path in PATHS:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                self.assertIn(GROUP, plan['groups'])
                self.assertIn('test_backup_published_original_ci', plan['python_tests'])
                self.assertCountEqual(origin_selections(plan, GROUP),
                                      self.policy['groups'][GROUP]['rust_targets'])
        assert_path_group_extension(self, self.policy, PATHS, None, [GROUP])

    def test_exact_real_source_default_and_fault_boundary_stays_bounded(self):
        default, faults, product, lean = self.policy['groups'][GROUP]['rust_targets']
        for target in (default, faults):
            self.assertEqual(target['exact'], [EXACT])
            self.assertEqual((target['kind'], target['max_tests'], target['timeout_seconds']),
                             ('lib', 1, 900))
            self.assertTrue(target['default_features'])
            self.assertTrue(target['isolate'])
            self.assertNotIn('prefixes', target)
            self.assertNotIn('whole', target)
        self.assertEqual(default['features'], [])
        self.assertEqual(faults['features'], ['fault-injection'])
        self.assertEqual([product['mode'], lean['mode']], ['check', 'check'])
        self.assertEqual([product['default_features'], lean['default_features']], [True, False])

    def test_existing_originals_and_terminal_gate_are_not_replaced(self):
        legacy = self.policy['groups']['backup-database-snapshot']['rust_targets'][0]
        self.assertEqual(legacy['prefixes'], ['container_storage::backup_original_objects::tests::'])
        self.assertEqual((legacy['max_tests'], legacy['timeout_seconds']), (32, 900))
        terminal = self.policy['groups']['terminal-backup-source-layout']['rust_targets'][:2]
        for target in terminal:
            self.assertEqual(target['name'], 'recursive_backup_paths')
            self.assertEqual(target['exact'], [
                'backup_preparation::transaction_serving::'
                'same_backup_switch_serves_and_replays_normal_successor_publications',
            ])
            self.assertEqual((target['max_tests'], target['timeout_seconds']), (1, 900))
