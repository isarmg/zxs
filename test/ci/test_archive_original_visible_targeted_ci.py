"""Keep original archive bytes and visible recovery outcomes on the real paths."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "archive_original_visible_targeted_ci", ROOT / ".github/scripts/check-targeted-ci.py")
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)

PATHS = [
    'src/engine/src/engine/membership_transition/archive/namespace_replay.rs',
    'src/engine/src/engine/membership_transition/pending/activation/exit/replay.rs',
    'src/engine/src/engine/membership_transition/pending/activation/backup/startup.rs',
    'test/engine/unit/engine/membership_control_archive_tests.rs',
    'test/ci/test_archive_original_visible_targeted_ci.py',
]
EXACT = [
    'engine::membership_transition::archive::persistent::tests::'
    'membership_control_archive_preserves_reprotect_plan_and_original_snapshot',
    'engine::membership_transition::archive::persistent::tests::'
    'membership_control_archive_is_independent_and_preserves_original_bytes',
    'engine::membership_transition::archive::persistent::tests::'
    'membership_control_archive_rejects_missing_old_graph_and_independent_finalize',
    'engine::disk_registry::current::tests::workspace_overlap::'
    'membership_registry_workspace_overlap_actual_d3_preserves_target_and_cold_restart',
]


class ArchiveOriginalVisibleSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_each_actual_path_selects_archive_runtime_and_keeps_existing_consumers(self):
        for path in PATHS:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                self.assertIn('archive-original-visible', plan['groups'])
                self.assertIn('test_archive_original_visible_targeted_ci', plan['python_tests'])
                if path.endswith('activation/exit/replay.rs'):
                    self.assertIn('container-tags-engine', plan['groups'])
                    self.assertIn('held-directory-layout', plan['groups'])
        unrelated = CI.select_plan(
            self.policy, ['src/engine/src/business/workspace/version_build.rs'], shardable=True)
        self.assertNotIn('archive-original-visible', unrelated['groups'])
        self.assertIn('workspace-version-construction', unrelated['groups'])

    def test_archive_and_backup_exact_tests_keep_features_and_separate_product_checks(self):
        group = self.policy['groups']['archive-original-visible']
        runtime = [target for target in group['rust_targets']
                   if target.get('mode', 'test') == 'test']
        self.assertEqual(len(runtime), 2)
        self.assertEqual(runtime[0]['exact'], EXACT)
        self.assertNotIn('prefixes', runtime[0])
        self.assertEqual(runtime[0]['features'], ['fault-injection'])
        self.assertEqual(runtime[0]['max_tests'], 4)
        backup = runtime[1]
        self.assertEqual(backup['package'], 'storage-engine')
        self.assertEqual(backup['kind'], 'test')
        self.assertEqual(backup['name'], 'recursive_backup_paths')
        self.assertTrue(backup['isolate'])
        self.assertEqual(backup['features'], ['fault-injection'])
        self.assertEqual(backup['exact'], [
            'backup_preparation::transaction_serving::'
            'same_backup_switch_serves_and_replays_normal_successor_publications',
        ])
        self.assertNotIn('prefixes', backup)
        self.assertEqual(backup['max_tests'], 1)
        self.assertEqual(backup['timeout_seconds'], 900)
        checks = [target for target in group['rust_targets'] if target.get('mode') == 'check']
        self.assertEqual(
            {(target['package'], target.get('default_features', True)) for target in checks},
            {('storage-engine', True), ('storage-engine', False)},
        )
        self.assertTrue(all(not target.get('features') for target in checks))


if __name__ == '__main__':
    unittest.main()
