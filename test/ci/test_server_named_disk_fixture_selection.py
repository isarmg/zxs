"""Keep the core test compile and named-disk fixture checks precise."""
import importlib.util
import json
from pathlib import Path
import unittest
from test_targeted_ci import assert_path_group_extension

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py',
)
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class ServerNamedDiskFixtureSelection(unittest.TestCase):
    def test_unmapped_fixture_paths_select_the_core_compile_and_exact_name_checks(self):
        policy = json.loads((ROOT / '.github/ci/targeted-tests.json').read_text())
        group = 'server-named-disk-fixtures'
        probe = 'test/ci/test_server_named_disk_fixture_selection.py'
        paths = [rule['path'] for rule in policy['rules'] if rule['groups'] == [group]]
        self.assertEqual(len(paths), 39)
        self.assertIn(probe, paths)
        self.assertIn('test/server/support/foreign_disk_fixture.rs', paths)
        for path in paths:
            with self.subTest(path=path):
                if path == 'test/server/unit/setup_recovery/activation_tests.rs':
                    plan = assert_path_group_extension(
                        self, policy, [path], [group], ['initial-storage-result-admission'],
                    )
                else:
                    plan = CI.select_plan(policy, [path])
                self.assertEqual(plan['groups'], [group])
                self.assertEqual(plan['python_tests'], [
                    'test_server_named_disk_fixture_selection',
                ])
                tests = [target for target in plan['rust_targets']
                         if target.get('mode', 'test') == 'test']
                checks = [target for target in plan['rust_targets'] if target.get('mode') == 'check']
                self.assertEqual(len(tests), 1)
                self.assertEqual(tests[0]['features'], ['core'])
                self.assertEqual(tests[0]['max_tests'], 2)
                self.assertEqual(tests[0]['exact'], [
                    'backend_storage_metadata::tests::disk_directory_names_are_exact_portable_basenames',
                    'backend_storage_metadata::tests::disk_directory_names_preserve_order_and_check_backup_roots',
                ])
                self.assertCountEqual(
                    [(target['package'], target['default_features']) for target in checks],
                    [('storage-server', True), ('storage-server', False)],
                )
