"""Select only the orphan export-wrapper retirement and all registered consumers."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py'
)
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)

GROUP = 'directory-export-wrapper-retirement'
PATHS = [
    'src/engine/src/business/directory_git/export.rs',
    'test/ci/test_directory_export_retirement_ci.py',
    'docs/design/directory-export-wrapper-retirement.md',
]


class DirectoryExportRetirement(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_each_path_selects_the_complete_retirement_contract(self):
        expected = self.policy['groups'][GROUP]['rust_targets']
        for path in PATHS:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                expected_groups = ['ci-selection', GROUP] if path.startswith('docs/') else [GROUP]
                self.assertEqual(plan['groups'], expected_groups)
                actual = [
                    {key: value for key, value in target.items()
                     if key != 'selection_groups'}
                    for target in plan['rust_targets']
                ]
                self.assertEqual(actual, expected)
                if path.startswith('docs/'):
                    self.assertEqual(len(plan['python_tests']), 8)
                    self.assertIn('test_targeted_ci', plan['python_tests'])
                    self.assertIn('test_directory_export_retirement_ci', plan['python_tests'])
                else:
                    self.assertEqual(plan['python_tests'], ['test_directory_export_retirement_ci'])

    def test_complete_change_preserves_scope_and_execution_budgets(self):
        plan = CI.select_plan(self.policy, PATHS + [
            '.github/ci/targeted-tests.json', 'TASKS.md',
        ])
        self.assertEqual(plan['groups'], ['ci-selection', GROUP])
        self.assertEqual(len(plan['rust_targets']), 8)
        self.assertEqual(len(plan['python_tests']), 8)
        self.assertTrue(plan['unexecuted'])
        tests = [target for target in plan['rust_targets'] if target.get('mode') != 'check']
        self.assertEqual([target['max_tests'] for target in tests], [1, 3, 2])
        self.assertTrue(all(target['timeout_seconds'] == 900 for target in tests))
        self.assertTrue(all(target['features'] == ['fault-injection'] for target in tests))
        self.assertIn(
            'rename::directory_git_rename_committed_export_rejects_without_publication',
            tests[1]['exact'],
        )
        self.assertIn(
            'rename::directory_git_rename_uncommitted_round_trip_keeps_history_tags_and_container',
            tests[1]['exact'],
        )
        self.assertIn(
            'history_copy_failure_boundaries_keep_source_and_recover_one_operation',
            tests[2]['exact'],
        )

    def test_product_checks_cover_engine_and_every_registered_runtime_consumer(self):
        plan = CI.select_plan(self.policy, PATHS)
        checks = [target for target in plan['rust_targets'] if target.get('mode') == 'check']
        self.assertEqual(
            {(target['package'], target.get('kind', 'lib'), target.get('name'))
             for target in checks},
            {
                ('storage-engine', 'lib', None),
                ('storage-server', 'lib', None),
                ('storage-helper', 'bin', 'zxs-helper'),
                ('perf-check', 'bin', 'perf-check'),
                ('browser-smoke', 'bin', 'browser-smoke'),
            },
        )
        engine = next(target for target in checks if target['package'] == 'storage-engine')
        self.assertFalse(engine['default_features'])
        self.assertNotIn('features', engine)
        server = next(target for target in checks if target['package'] == 'storage-server')
        self.assertEqual(server['features'], ['core'])
        self.assertTrue(all('fault-injection' not in target.get('features', []) for target in checks))


if __name__ == '__main__':
    unittest.main()
