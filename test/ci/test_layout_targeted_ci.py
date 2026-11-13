"""Exercise the layout scope with measured module sizes and a strict execution cap."""
import importlib.util
import json
from pathlib import Path
import unittest
from test_targeted_ci import assert_path_group_extension

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'layout_targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)

# Compiled inventory: run 43227554097, artifact 13954370354, SHA 09b7715f3.
# All 190 runnable names remain selected. Two first-slot repair regressions add
# behavior, so readmission now has its own 21-case cap beside the retained 171.
# New counts are static source evidence; CI must re-enumerate the current binary.
# Record module sizes, not test names, so legitimate behavior renames remain possible.
# Each tuple is (prefix, runnable count, ignored count).
MODULE_SIZES = [
    ('persist::layout::tests::', 4, 0),
    ('disk::owned::root_operation_tests::', 10, 0),
    ('disk::owned::directory_entries_tests::', 9, 0),
    ('disk::admission_tests::', 3, 2),
    ('journal::writer_lifecycle_tests::', 9, 0),
    ('journal::writer_tests::', 2, 0),
    ('engine::membership_transition::readmission::', 21, 0),
    ('engine::membership_transition::pending::terminal_resume::', 47, 1),
    ('engine::membership_transition::pending::activation::backup::startup::resume::', 4, 0),
    ('engine::backup_preparation::tests::', 7, 0),
    ('engine::recovery_checkpoint::tests::', 53, 0),
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
        group = self.policy['groups']['held-directory-layout']
        selected = CI.select_plan({
            'groups': {'held-directory-layout': group},
            'rules': [{'path': 'layout.rs', 'groups': ['held-directory-layout']}],
            'unexecuted': self.policy['unexecuted'],
        }, ['layout.rs'])
        self.target = next(target for target in selected['rust_targets']
                           if target['package'] == 'storage-engine')
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
        self.assertEqual(len(selected), 192)
        self.assertEqual([part['max_tests'] for part in self.target['selections']], [171, 21])
        self.assertEqual(self.target['selection_groups'],
                         ['held-directory-layout', 'held-directory-layout'])
        self.assertEqual(len(self.ignored), 3)

    def test_one_more_selected_test_requires_review(self):
        for prefix in ['persist::layout::tests::',
                       'engine::membership_transition::readmission::']:
            names = self.runnable + [prefix + 'additional_behavior']
            with self.subTest(prefix=prefix), self.assertRaises(ValueError):
                CI.selected_tests(names, [], self.target)

    def test_missing_recovery_module_cannot_pass_within_budget(self):
        for prefix in ['engine::recovery_checkpoint::tests::',
                       'engine::membership_transition::readmission::']:
            names = [name for name in self.runnable if not name.startswith(prefix)]
            with self.subTest(prefix=prefix), self.assertRaises(ValueError):
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
        plan = assert_path_group_extension(self, self.policy,
            ['src/engine/src/persist/layout.rs'], None, ['container-manual-fresh-format'])
        self.assertEqual(plan['groups'],
                         ['checkpoint-layout', 'held-directory-layout', 'projection-cache-layout'])
        engine = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-engine'
                      and target.get('mode', 'test') == 'test' and not target.get('features'))
        self.assertEqual([item['max_tests'] for item in engine['selections']], [171, 21, 52])
        layout = [f'{prefix}behavior_{index}'
                  for prefix, count, _ in MODULE_SIZES for index in range(count)]
        selected = CI.selected_tests(layout + self.runnable + self.ignored,
                                     self.ignored, engine)
        self.assertEqual(selected, sorted(layout + self.runnable))
        self.assertEqual(len(selected), 244)
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


class CheckpointLayoutTargetedCi(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_product_check_and_explicit_injection_tests_keep_separate_builds(self):
        paths = [
            'src/engine/src/engine/recovery_checkpoint.rs',
            'test/engine/unit/engine/recovery_checkpoint/tests.rs',
            'test/engine/unit/engine/recovery_checkpoint/layout_tests.rs',
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'], ['checkpoint-layout', 'held-directory-layout'])
                product = next(target for target in plan['rust_targets']
                               if target.get('mode') == 'check')
                self.assertEqual(product['package'], 'storage-engine')
                self.assertFalse(product['default_features'])
                self.assertNotIn('features', product)
                injected = next(target for target in plan['rust_targets']
                                if target.get('features') == ['fault-injection'])
                self.assertEqual(injected['prefixes'], ['engine::recovery_checkpoint::tests::'])
                self.assertEqual(injected['max_tests'], 53)
                self.assertEqual(injected['timeout_seconds'], 900)

    def test_all_changed_paths_keep_every_original_overlap_within_the_build_limit(self):
        paths = [
            '.github/ci/targeted-tests.json',
            'src/engine/src/disk/owned.rs',
            'src/engine/src/persist/layout.rs',
            'src/engine/src/engine/recovery_checkpoint.rs',
            'test/engine/unit/engine/recovery_checkpoint/tests.rs',
            'test/engine/unit/engine/recovery_checkpoint/layout_tests.rs',
            'test/ci/test_layout_targeted_ci.py',
            'test/ci/test_targeted_ci.py',
        ]
        plan = assert_path_group_extension(self, self.policy, paths, None,
                                           ['container-manual-fresh-format', 'container-manual-drafts',
                                            'container-manual-drafts-consumers',
                                            'container-manual-recovery',
                                            'manual-body-retirement-consumers'])
        self.assertEqual(plan['groups'],
                         ['checkpoint-layout', 'ci-selection', 'held-directory-layout',
                          'ordinary-conversion-name-budget', 'projection-cache-layout'])
        self.assertEqual(len(plan['rust_targets']), 6)
        engine = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-engine'
                      and target.get('mode', 'test') == 'test' and not target.get('features'))
        self.assertEqual(engine['selection_groups'],
                         ['held-directory-layout', 'held-directory-layout',
                          'ordinary-conversion-name-budget',
                          'projection-cache-layout'])
        self.assertEqual([item['max_tests'] for item in engine['selections']], [171, 21, 22, 52])
        self.assertEqual([item.get('timeout_seconds', 300) for item in engine['selections']],
                         [300, 300, 900, 300])
        platform = next(target for target in plan['rust_targets']
                        if target['package'] == 'platform-io')
        self.assertEqual(platform['max_tests'], 100)
        server = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-server'
                      and target.get('mode', 'test') == 'test')
        self.assertEqual(server['max_tests'], 20)
        self.assertEqual(server['features'], ['core'])


if __name__ == '__main__':
    unittest.main()
