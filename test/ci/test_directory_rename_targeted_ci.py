"""Keep directory rename recovery consumers and every overlapping budget selected."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'directory_rename_targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)

PATHS = [
    'src/web/src/files/actions/mutation.rs',
    'src/web/src/files/mkdir.rs',
    'src/web/src/files/mkdir/storage.rs',
    'src/web/src/files/mkdir/storage/directory.rs',
    'src/web/src/files/mkdir/view.rs',
    'test/web/unit/files/directory_branch_recovery_tests.rs',
    'test/web/unit/files/directory_rename_recovery_tests.rs',
]


class DirectoryRenameTargetedCi(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_all_changed_consumers_select_rename_recovery(self):
        for path in PATHS + ['test/ci/test_directory_rename_targeted_ci.py']:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('directory-rename-recovery', plan['groups'])
                self.assertIn('test_directory_rename_targeted_ci', plan['python_tests'])
                self.assertLessEqual(len(plan['rust_targets']), 16)

    def test_complete_candidate_retains_existing_independent_selection_budgets(self):
        plan = CI.select_plan(self.policy, PATHS + [
            '.github/ci/targeted-tests.json',
            'test/ci/test_directory_rename_targeted_ci.py',
            'docs/validation/directory-rename-recovery.md',
        ])
        self.assertIn('historical-directory-web', plan['groups'])
        self.assertIn('web-recovery-principals', plan['groups'])
        self.assertEqual(len(plan['rust_targets']), 4)
        self.assertLessEqual(len(plan['python_tests']), 8)
        native = next(target for target in plan['rust_targets']
                      if target['package'] == 'web-app'
                      and target.get('mode', 'test') == 'test')
        selections = dict(zip(native['selection_groups'], native['selections']))
        self.assertEqual(selections['historical-directory-web']['max_tests'], 56)
        self.assertEqual(selections['web-recovery-principals']['max_tests'], 131)
        self.assertEqual(selections['directory-rename-recovery']['max_tests'], 72)
        self.assertIn('files::mkdir::storage::tests::',
                      selections['historical-directory-web']['prefixes'])
        self.assertIn('files::mkdir::storage::tests::',
                      selections['web-recovery-principals']['prefixes'])

    def test_product_compilation_is_separate_from_native_behavior_and_browser_acceptance(self):
        group = self.policy['groups']['directory-rename-recovery']
        native, product, wasm, server = group['rust_targets']
        self.assertEqual(native['package'], 'web-app')
        self.assertIn('files::mkdir::storage::rename_tests::', native['prefixes'])
        self.assertIn('files::actions::mutation::tests::', native['prefixes'])
        self.assertIn('files::actions::operation::tests::', native['prefixes'])
        self.assertIn('files::inline::tests::', native['prefixes'])
        self.assertEqual(product['mode'], 'check')
        self.assertNotIn('compile_target', product)
        self.assertEqual(wasm['mode'], 'check')
        self.assertEqual(wasm['compile_target'], 'wasm32-unknown-unknown')
        for target in (product, wasm):
            self.assertNotIn('max_tests', target)
            self.assertNotIn('prefixes', target)
            self.assertNotIn('--no-run', CI.cargo_command(target, '/build'))
        self.assertEqual(server['features'], ['core'])
        self.assertEqual(server['max_tests'], 2)
        self.assertEqual(server['exact'], [
            'http_default_directory::tests::'
            'rename_ingress_binds_both_paths_and_keeps_operation_before_name_policy',
            'engine_backend::read_projection::tests::default_directory_tests::'
            'typed_directory_rename_keeps_canonical_receipts_and_cached_name_collision_policy',
        ])


if __name__ == '__main__':
    unittest.main()
