"""Exercise change selection and fail-closed runtime evidence with synthetic inputs."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class TargetedSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_tooling_change_selects_only_related_python_modules(self):
        plan = CI.select_plan(self.policy, ['.github/scripts/check-targeted-ci.py'])
        self.assertEqual(plan['rust_targets'], [])
        self.assertIn('test_targeted_ci', plan['python_tests'])
        self.assertTrue(plan['unexecuted'])

    def test_layout_and_ci_changes_union_groups_once(self):
        plan = CI.select_plan(self.policy, ['src/engine/src/persist/layout.rs',
                            'src/engine/src/journal.rs', '.github/workflows/ci.yml'])
        self.assertEqual(len(plan['rust_targets']), 2)
        self.assertEqual(len(plan['groups']), 2)
        self.assertIn('test_development_ci', plan['python_tests'])

    def test_unknown_empty_and_documentation_only_changes_need_target_selection(self):
        for changed in ([], ['src/server/src/lib.rs']):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                CI.select_plan(self.policy, changed)

    def test_malformed_or_unbounded_target_is_rejected(self):
        for field, value in [('prefixes', []), ('max_tests', 0), ('max_tests', 161),
                             ('prefixes', ['']), ('package', '--workspace')]:
            policy = copy.deepcopy(self.policy)
            policy['groups']['held-directory-layout']['rust_targets'][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                CI.select_plan(policy, ['src/engine/src/persist/layout.rs'])

    def test_current_inventory_resolves_new_test_names_and_excludes_ignored(self):
        target = {'prefixes': ['layout::'], 'max_tests': 2}
        names = ['layout::renamed_behavior', 'layout::ignored', 'other::test']
        self.assertEqual(CI.selected_tests(names, ['layout::ignored'], target),
                         ['layout::renamed_behavior'])

    def test_empty_module_ignored_only_and_over_budget_fail(self):
        for names, ignored, maximum in [([], [], 2), (['layout::x'], ['layout::x'], 2),
                                        (['layout::x', 'layout::y'], [], 1)]:
            with self.subTest(names=names), self.assertRaises(ValueError):
                CI.selected_tests(names, ignored, {'prefixes': ['layout::'], 'max_tests': maximum})

    def test_exact_nonzero_test_result_is_required(self):
        line = 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 20 filtered out; finished in 0.1s\n'
        CI.passed_test(line, 21)
        for invalid in ['', line.replace('1 passed', '0 passed'),
                        line.replace('0 ignored', '1 ignored'), line.replace('20 filtered', '19 filtered')]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                CI.passed_test(invalid, 21)

    def test_namespace_child_summary_uses_the_final_parent_inventory(self):
        child = 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s\n'
        parent = child.replace('0 filtered', '20 filtered')
        CI.passed_test(child + parent, 21)

    def test_documentation_change_runs_lightweight_policy_regressions(self):
        plan = CI.select_plan(self.policy, ['TASKS.md'])
        self.assertEqual(plan['rust_targets'], [])
        self.assertTrue(plan['python_tests'])

    def test_python_zero_tests_missing_import_and_skips_cannot_pass(self):
        CI.passed_python('Ran 2 tests in 0.001s\n\nOK\n', 2)
        for output in ['Ran 0 tests in 0.001s\n\nOK\n',
                       'Ran 1 test in 0.001s\n\nFAILED (errors=1)\n',
                       'Ran 1 test in 0.001s\n\nOK (skipped=1)\n']:
            with self.assertRaises(ValueError):
                CI.passed_python(output, 1)

    def test_invalid_inventory_is_rejected(self):
        self.assertEqual(CI.inventory('module::test: test\n\n1 test, 0 benchmarks\n'), ['module::test'])
        for invalid in ['unexpected output', 'x: test\nx: test\n']:
            with self.assertRaises(ValueError):
                CI.inventory(invalid)


if __name__ == '__main__':
    unittest.main()
