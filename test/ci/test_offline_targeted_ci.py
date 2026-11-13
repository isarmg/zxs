"""Keep offline slot admission scoped to its changed boundary and consumers."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'offline_targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class OfflineTargetedCi(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_engine_export_keeps_original_selections_and_adds_conversion_budget(self):
        plan = CI.select_plan(self.policy, ['src/engine/src/lib.rs'])
        self.assertEqual(plan['groups'], ['container-lifecycle-engine',
                                         'container-path-facts-engine',
                                         'held-directory-layout',
                                         'ordinary-conversion-name-budget',
                                         'typed-name-consumers'])
        self.assertEqual(len(plan['rust_targets']), 13)
        for group in plan['groups']:
            selections = []
            for target in plan['rust_targets']:
                configurations = target.get('selections', [
                    {key: value for key, value in target.items() if key != 'selection_groups'}
                ])
                selections.extend(configuration for origin, configuration
                                  in zip(target['selection_groups'], configurations)
                                  if origin == group)
            self.assertCountEqual(selections, self.policy['groups'][group]['rust_targets'])

    def test_offline_paths_select_behavior_and_explicit_consumer_checks(self):
        for path in ['src/server/src/offline/foreign_chain.rs',
                     'test/server/unit/offline/foreign_chain_tests.rs']:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'], ['offline-chain-slots'])
                self.assertEqual(len(plan['rust_targets']), 5)
                test = next(target for target in plan['rust_targets']
                            if target.get('mode', 'test') == 'test')
                self.assertEqual((test['package'], test['kind'], test['name']),
                                 ('storage-server', 'bin', 'zxs'))
                self.assertEqual(test['features'], ['core'])
                self.assertEqual(test['prefixes'], ['offline::foreign_chain::tests::'])
                self.assertEqual(test['max_tests'], 12)
                checks = {(target['package'], target.get('name'))
                          for target in plan['rust_targets'] if target.get('mode') == 'check'}
                self.assertEqual(checks, {('storage-engine', None),
                                         ('storage-helper', 'zxs-helper'),
                                         ('perf-check', 'perf-check'),
                                         ('perf-check', 'recovery-startup')})

    def test_regression_change_runs_policy_and_offline_scope(self):
        plan = CI.select_plan(self.policy, ['test/ci/test_offline_targeted_ci.py'])
        self.assertEqual(plan['groups'], ['ci-selection', 'offline-chain-slots'])
        self.assertIn('test_offline_targeted_ci', plan['python_tests'])
        self.assertIn('test_targeted_ci', plan['python_tests'])
        self.assertEqual(len(plan['rust_targets']), 5)


if __name__ == '__main__':
    unittest.main()
