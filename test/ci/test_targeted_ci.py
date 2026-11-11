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

    def test_unknown_and_empty_changes_need_target_selection(self):
        for changed in ([], ['__unmapped_fixture__/unknown.rs']):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                CI.select_plan(self.policy, changed)

    def test_malformed_or_unbounded_target_is_rejected(self):
        for field, value in [('prefixes', []), ('max_tests', 0), ('max_tests', 193),
                             ('prefixes', ['']), ('package', '--workspace'), ('timeout_seconds', 901)]:
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

    def test_named_core_integration_build_is_explicit(self):
        target = {'package': 'storage-server', 'manifest': 'src/server/Cargo.toml',
                  'kind': 'test', 'name': 'phase1_smb_rpc', 'features': ['core'],
                  'prefixes': ['http_soft_deletion::'], 'max_tests': 30}
        command = CI.cargo_command(target, '/build')
        self.assertIn('--test', command)
        self.assertIn('phase1_smb_rpc', command)
        self.assertIn('--no-run', command)
        self.assertEqual(command[command.index('--features') + 1], 'core')
        self.assertNotIn('--lib', command)

    def test_wasm_check_and_native_bin_check_do_not_claim_tests(self):
        target = {'package': 'web-app', 'mode': 'check', 'compile_target': 'wasm32-unknown-unknown'}
        command = CI.cargo_command(target, '/build')
        self.assertEqual(command[:2], ['cargo', 'check'])
        self.assertIn('--release', command)
        self.assertIn('wasm32-unknown-unknown', command)
        self.assertNotIn('--no-run', command)
        binary = CI.cargo_command({'package': 'perf-check', 'mode': 'check',
                                  'kind': 'bin', 'name': 'recovery-startup'}, '/build')
        self.assertIn('recovery-startup', binary)
        self.assertNotIn('--all-targets', binary)

    def test_exact_selection_must_exist_and_be_runnable(self):
        target = {'exact': ['module::test'], 'max_tests': 2}
        self.assertEqual(CI.selected_tests(['module::test'], [], target), ['module::test'])
        for names, ignored in [([], []), (['module::test'], ['module::test'])]:
            with self.assertRaises(ValueError):
                CI.selected_tests(names, ignored, target)

    def test_artifact_kind_name_features_manifest_and_completion_are_bound(self):
        target = {'package': 'web-app', 'manifest': 'src/web/Cargo.toml'}
        item = {'reason': 'compiler-artifact', 'target': {'name': 'web_app',
                'kind': ['cdylib', 'rlib']}, 'manifest_path': str(ROOT / target['manifest']),
                'profile': {'test': True}, 'features': [], 'executable': '/build/web_app'}
        def output(artifact, success=True):
            return json.dumps(artifact) + '\n' + json.dumps({'reason': 'build-finished', 'success': success})
        self.assertEqual(CI.current_artifact(output(item), target, ROOT), item)
        for mutation in [{'target': {'name': 'other', 'kind': ['lib']}},
                         {'manifest_path': '/different/Cargo.toml'},
                         {'profile': {'test': False}}]:
            with self.assertRaises(ValueError):
                CI.current_artifact(output(dict(item, **mutation)), target, ROOT)
        with self.assertRaises(ValueError):
            CI.current_artifact(output(item, False), target, ROOT)
        with self.assertRaises(ValueError):
            CI.current_artifact(output(item), dict(target, features=['core']), ROOT)

    def overlapping_policy(self, first, second):
        return {'groups': {
            'first': {'reason': 'first behavior', 'python_tests': [], 'rust_targets': [first]},
            'second': {'reason': 'second behavior', 'python_tests': [], 'rust_targets': [second]}},
            'rules': [{'path': 'change', 'groups': ['first', 'second']}],
            'unexecuted': 'other behaviors'}

    def test_overlapping_build_keeps_separate_budgets_and_strictest_timeout(self):
        first = {'package': 'storage-engine', 'manifest': 'src/engine/Cargo.toml',
                 'prefixes': ['left::'], 'max_tests': 2, 'timeout_seconds': 900}
        second = dict(first, prefixes=['right::'], exact=['left::shared'],
                      max_tests=2, timeout_seconds=120)
        plan = CI.select_plan(self.overlapping_policy(first, second), ['change'])
        self.assertEqual(len(plan['rust_targets']), 1)
        target = plan['rust_targets'][0]
        names = ['left::shared', 'left::only', 'right::only', 'other::ignored']
        evidence = CI.selection_evidence(names, ['other::ignored'], target)
        self.assertEqual([item['group'] for item in evidence], ['first', 'second'])
        self.assertEqual([item['configuration'] for item in evidence], [first, second])
        self.assertEqual(CI.test_timeouts(evidence),
                         {'left::shared': 120, 'left::only': 900, 'right::only': 120})
        self.assertEqual(CI.selected_tests(names, ['other::ignored'], target),
                         ['left::only', 'left::shared', 'right::only'])
        self.assertEqual(first['max_tests'], 2)

    def test_overlap_cannot_mask_missing_ignored_or_over_budget_selection(self):
        first = {'package': 'storage-engine', 'manifest': 'src/engine/Cargo.toml',
                 'whole': True, 'max_tests': 10}
        for second, names, ignored in [
            (dict(first, whole=False, prefixes=['missing::']), ['other::x'], []),
            (dict(first, whole=False, exact=['other::x']), ['other::x', 'other::y'],
             ['other::x']),
            (dict(first, max_tests=1), ['other::x', 'other::y'], [])]:
            with self.subTest(second=second):
                target = CI.select_plan(self.overlapping_policy(first, second),
                                        ['change'])['rust_targets'][0]
                with self.assertRaises(ValueError):
                    CI.selection_evidence(names, ignored, target)

    def test_build_identity_normalizes_feature_order_but_preserves_graphs(self):
        first = {'package': 'storage-server', 'manifest': 'src/server/Cargo.toml',
                 'mode': 'check', 'features': ['core', 'extra']}
        reordered = dict(first, features=['extra', 'core'])
        plan = CI.select_plan(self.overlapping_policy(first, reordered), ['change'])
        self.assertEqual(len(plan['rust_targets']), 1)
        self.assertEqual(plan['rust_targets'][0]['selection_groups'], ['first', 'second'])
        for mutation in [{'features': ['core']}, {'default_features': False},
                         {'compile_target': 'wasm32-unknown-unknown'},
                         {'kind': 'bin', 'name': 'zxs'},
                         {'mode': 'test', 'whole': True, 'max_tests': 2}]:
            with self.subTest(mutation=mutation):
                plan = CI.select_plan(self.overlapping_policy(first, dict(first, **mutation)),
                                      ['change'])
                self.assertEqual(len(plan['rust_targets']), 2)
        with self.assertRaises(ValueError):
            CI.select_plan(self.overlapping_policy(first, dict(first, manifest='helper/Cargo.toml')),
                           ['change'])

    def test_unique_build_cap_applies_after_overlap_coalescing(self):
        target = {'package': 'storage-engine', 'manifest': 'src/engine/Cargo.toml',
                  'whole': True, 'max_tests': 2}
        policy = self.overlapping_policy(target, dict(target))
        policy['groups']['first']['rust_targets'] = [dict(target) for _ in range(17)]
        self.assertEqual(len(CI.select_plan(policy, ['change'])['rust_targets']), 1)
        policy['groups']['first']['rust_targets'] = [
            dict(target, kind='test', name=f'target_{index}') for index in range(16)]
        with self.assertRaises(ValueError):
            CI.select_plan(policy, ['change'])
        policy['groups']['first']['rust_targets'] = [dict(target, max_tests=193)]
        with self.assertRaises(ValueError):
            CI.select_plan(policy, ['change'])

    def test_path_consumer_repairs_and_layout_policy_share_fifteen_builds(self):
        plan = CI.select_plan(self.policy, ['src/engine/src/business/directory.rs',
                            'src/engine/src/business/workspace.rs',
                            'test/ci/test_layout_targeted_ci.py'])
        self.assertEqual(plan['groups'], ['ci-selection', 'held-directory-layout',
                                         'portable-path-consumers'])
        self.assertEqual(len(plan['rust_targets']), 15)
        engine = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-engine' and target.get('kind', 'lib') == 'lib')
        self.assertEqual(engine['selection_groups'],
                         ['held-directory-layout', 'portable-path-consumers'])
        self.assertEqual([selection['max_tests'] for selection in engine['selections']], [187, 160])

    def test_invalid_inventory_is_rejected(self):
        self.assertEqual(CI.inventory('module::test: test\n\n1 test, 0 benchmarks\n'), ['module::test'])
        for invalid in ['unexpected output', 'x: test\nx: test\n']:
            with self.assertRaises(ValueError):
                CI.inventory(invalid)


if __name__ == '__main__':
    unittest.main()
