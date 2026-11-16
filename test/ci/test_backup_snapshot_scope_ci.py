"""Whole Backup must not silently omit a permanently registered .zxs snapshot."""
import json
from pathlib import Path
import unittest

from test_targeted_ci import CI, assert_path_group_extension, origin_selections

ROOT = Path(__file__).resolve().parents[2]
GROUP = 'backup-snapshot-scope'
PREFIX = 'snapshot_worktree::registry::backup::tests::'
EXACT = [
    PREFIX + 'published_snapshot_blocks_whole_backup_before_intent_or_target_writes',
    PREFIX + 'reserved_snapshot_is_pending_not_a_published_snapshot',
]
PATHS = [
    'src/engine/src/engine/backup_preparation.rs',
    'src/engine/src/engine/backup_preparation/snapshot.rs',
    'src/engine/src/snapshot_worktree.rs',
    'src/engine/src/snapshot_worktree/source.rs',
    'src/engine/src/snapshot_worktree/registry.rs',
    'src/engine/src/snapshot_worktree/registry/backup.rs',
    'test/engine/unit/backup_snapshot_scope_tests.rs',
    'test/ci/test_backup_snapshot_scope_ci.py',
    'docs/validation/backup-snapshot-scope.md',
]


class BackupSnapshotScopeSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_every_new_gate_and_actual_fixture_selects_both_configurations(self):
        for path in PATHS:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                self.assertIn('test_backup_snapshot_scope_ci', plan['python_tests'])
                self.assertCountEqual(origin_selections(plan, GROUP),
                                      self.policy['groups'][GROUP]['rust_targets'])
        assert_path_group_extension(self, self.policy, PATHS, None, [GROUP])

    def test_exact_genuine_registration_tests_are_bounded_not_wide_prefixes(self):
        default, faults, product, lean = self.policy['groups'][GROUP]['rust_targets']
        for target in (default, faults):
            self.assertEqual(target['kind'], 'lib')
            self.assertEqual(target['exact'], EXACT)
            self.assertEqual(target['max_tests'], 2)
            self.assertEqual(target['timeout_seconds'], 300)
            self.assertTrue(target['default_features'])
            self.assertTrue(target['isolate'])
            self.assertNotIn('prefixes', target)
            self.assertNotIn('whole', target)
        self.assertEqual(default['features'], [])
        self.assertEqual(faults['features'], ['fault-injection'])
        self.assertEqual([product['mode'], lean['mode']], ['check', 'check'])
        self.assertEqual([product['default_features'], lean['default_features']], [True, False])

    def test_actual_backup_entry_keeps_original_terminal_900_and_previous_groups(self):
        path = 'src/engine/src/engine/backup_preparation.rs'
        plan = CI.select_plan(self.policy, [path], shardable=True)
        original = 'terminal-backup-source-layout'
        self.assertCountEqual(origin_selections(plan, original),
                              self.policy['groups'][original]['rust_targets'])
        targets = self.policy['groups'][original]['rust_targets'][:2]
        for target in targets:
            self.assertEqual(target['exact'], [
                'backup_preparation::transaction_serving::'
                'same_backup_switch_serves_and_replays_normal_successor_publications'])
            self.assertEqual((target['max_tests'], target['timeout_seconds']), (1, 900))
        self.assertEqual([target['features'] for target in targets], [[], ['fault-injection']])


if __name__ == '__main__':
    unittest.main()
