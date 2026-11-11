"""Exercise the layout scope with measured module sizes and a strict execution cap."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'layout_targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)

# Compiled inventory: run 42988331435, artifact 13840114727, SHA 6b311c0d.
# 190 selected names include three ignored tests; all 187 runnable tests remain selected.
# Record module sizes, not test names, so legitimate behavior renames remain possible.
# Each tuple is (prefix, runnable count, ignored count).
MODULE_SIZES = [
    ('persist::layout::tests::', 4, 0),
    ('disk::owned::root_operation_tests::', 10, 0),
    ('disk::owned::directory_entries_tests::', 9, 0),
    ('disk::admission_tests::', 3, 2),
    ('journal::writer_lifecycle_tests::', 9, 0),
    ('journal::writer_tests::', 2, 0),
    ('engine::membership_transition::readmission::', 19, 0),
    ('engine::membership_transition::pending::terminal_resume::', 47, 1),
    ('engine::membership_transition::pending::activation::backup::startup::resume::', 4, 0),
    ('engine::backup_preparation::tests::', 7, 0),
    ('engine::recovery_checkpoint::tests::', 50, 0),
    ('engine::history_move_decision::choice::tests::', 6, 0),
    ('engine::backup_preparation::native_control::databases::scoped::cutover::'
     'registration::target_seals::store::tests::', 8, 0),
    ('engine::backup_preparation::native_control::databases::scoped::cutover::'
     'registration::target_writers::store::tests::', 8, 0),
    ('container_storage::tests::cross_directory_recovery::', 1, 0),
]


# Static source inventory at candidate 911c1d17; CI re-enumerates the compiled tests.
# Two journal interposer tests stay ignored in this ordinary targeted group.
CACHE_MODULE_SIZES = [
    ('disk_authority::cache::tests::', 8, 0),
    ('disk_authority::protocol_tests::', 27, 0),
    ('disk_authority::journal_tests::', 17, 2),
]


class LayoutTargetedCi(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())
        self.target = self.policy['groups']['held-directory-layout']['rust_targets'][0]
        self.runnable = [f'{prefix}behavior_{index}'
                         for prefix, count, _ in MODULE_SIZES for index in range(count)]
        self.ignored = [f'{prefix}ignored_{index}'
                        for prefix, _, count in MODULE_SIZES for index in range(count)]

    def test_specialized_regression_runs_product_and_policy_checks(self):
        plan = CI.select_plan(self.policy, ['test/ci/test_layout_targeted_ci.py'])
        self.assertEqual(plan['groups'],
                         ['ci-selection', 'held-directory-layout', 'projection-cache-layout'])
        self.assertIn('test_layout_targeted_ci', plan['python_tests'])
        self.assertIn('test_targeted_ci', plan['python_tests'])
        self.assertEqual({target['package'] for target in plan['rust_targets']},
                         {'storage-engine', 'platform-io'})

    def test_measured_scope_keeps_all_runnable_layout_and_recovery_tests(self):
        unrelated = ['other::unselected_behavior']
        selected = CI.selected_tests(self.runnable + self.ignored + unrelated,
                                     self.ignored, self.target)
        self.assertEqual(selected, sorted(self.runnable))
        self.assertEqual(len(selected), 187)
        self.assertEqual(self.target['max_tests'], 187)
        self.assertEqual(len(self.ignored), 3)

    def test_one_more_selected_test_requires_review(self):
        names = self.runnable + ['persist::layout::tests::additional_behavior']
        with self.assertRaises(ValueError):
            CI.selected_tests(names, [], self.target)

    def test_missing_recovery_module_cannot_pass_within_budget(self):
        names = [name for name in self.runnable
                 if not name.startswith('engine::recovery_checkpoint::tests::')]
        with self.assertRaises(ValueError):
            CI.selected_tests(names, [], self.target)


class ProjectionCacheTargetedCi(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())
        self.target = self.policy['groups']['projection-cache-layout']['rust_targets'][0]
        self.runnable = [f'{prefix}behavior_{index}'
                         for prefix, count, _ in CACHE_MODULE_SIZES for index in range(count)]
        self.ignored = [f'{prefix}ignored_{index}'
                        for prefix, _, count in CACHE_MODULE_SIZES for index in range(count)]

    def test_complete_cache_consumer_and_behavior_paths_select_the_cache_group(self):
        for path in [
            'src/engine/src/disk_authority/cache.rs',
            'src/engine/src/disk_authority/projection.rs',
            'test/engine/unit/disk_authority/cache_layout.rs',
            'test/engine/unit/disk_authority/protocol.rs',
            'test/engine/unit/disk_authority/journal.rs',
        ]:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'], ['projection-cache-layout'])
                selected = CI.selected_tests(self.runnable + self.ignored,
                                             self.ignored, plan['rust_targets'][0])
                self.assertEqual(selected, sorted(self.runnable))

    def test_shared_layout_keeps_both_independent_budgets_and_their_full_union(self):
        plan = CI.select_plan(self.policy, ['src/engine/src/persist/layout.rs'])
        self.assertEqual(plan['groups'], ['held-directory-layout', 'projection-cache-layout'])
        engine = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-engine')
        self.assertEqual([item['max_tests'] for item in engine['selections']], [187, 52])
        layout = [f'{prefix}behavior_{index}'
                  for prefix, count, _ in MODULE_SIZES for index in range(count)]
        selected = CI.selected_tests(layout + self.runnable + self.ignored,
                                     self.ignored, engine)
        self.assertEqual(selected, sorted(layout + self.runnable))
        self.assertEqual(len(selected), 239)
        self.assertEqual({target['package'] for target in plan['rust_targets']},
                         {'storage-engine', 'platform-io'})

    def test_cache_budget_excludes_the_two_ignored_interposer_cases(self):
        selected = CI.selected_tests(self.runnable + self.ignored, self.ignored, self.target)
        self.assertEqual(len(selected), 52)
        self.assertEqual(self.target['max_tests'], 52)
        self.assertEqual(len(self.ignored), 2)

    def test_an_extra_cache_behavior_requires_review_of_the_specific_budget(self):
        names = self.runnable + ['disk_authority::cache::tests::additional_behavior']
        with self.assertRaises(ValueError):
            CI.selected_tests(names, [], self.target)

    def test_each_original_disk_behavior_module_remains_required(self):
        for prefix, _, _ in CACHE_MODULE_SIZES:
            names = [name for name in self.runnable if not name.startswith(prefix)]
            with self.subTest(prefix=prefix), self.assertRaises(ValueError):
                CI.selected_tests(names, [], self.target)


if __name__ == '__main__':
    unittest.main()
