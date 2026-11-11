"""Exercise paged-deletion scope admission and exact HTTP target configuration."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('deletion_targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class DeletionTargetedCi(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_server_entrypoint_selects_registered_deletion_scope(self):
        plan = CI.select_plan(self.policy, ['src/server/src/lib.rs'])
        self.assertIn('paged-deletion-review', plan['groups'])
        self.assertEqual(len(plan['rust_targets']), 5)

    def test_specialized_regression_change_runs_product_and_policy_checks(self):
        plan = CI.select_plan(self.policy, ['test/ci/test_deletion_targeted_ci.py'])
        self.assertEqual(plan['groups'], ['ci-selection', 'paged-deletion-review'])
        self.assertIn('test_deletion_targeted_ci', plan['python_tests'])
        self.assertIn('test_targeted_ci', plan['python_tests'])
        self.assertEqual(len(plan['rust_targets']), 5)

    def test_http_acceptance_uses_core_integration_and_exact_durable_behavior(self):
        plan = CI.select_plan(self.policy, ['src/server/src/http_file_deletion/review.rs'])
        target = next(t for t in plan['rust_targets'] if t.get('name') == 'phase1_smb_rpc')
        self.assertEqual(target['kind'], 'test')
        self.assertIn('core', target['features'])
        self.assertEqual(target['prefixes'], ['http_soft_deletion::'])
        self.assertIn('soft_deletion::public_http_deletion_is_discoverable_and_undo_is_exact_authenticated_and_durable',
                      target['exact'])
        self.assertEqual(target['timeout_seconds'], 900)
        command = CI.cargo_command(target, '/build')
        self.assertIn('--no-run', command)
        self.assertNotIn('--lib', command)

    def test_web_native_behavior_and_wasm_compilation_have_distinct_evidence(self):
        plan = CI.select_plan(self.policy, ['src/web/src/files/deletion/summary.rs'])
        web = [t for t in plan['rust_targets'] if t['package'] == 'web-app']
        tests = next(t for t in web if t.get('mode', 'test') == 'test')
        compile_only = next(t for t in web if t.get('mode') == 'check')
        selections = tests.get('selections', [tests])
        deletion_selection = next(selection for selection in selections
                                  if selection.get('prefixes') ==
                                  ['files::deletion::', 'files::actions::operation::tests::'])
        self.assertEqual(deletion_selection['prefixes'],
                         ['files::deletion::', 'files::actions::operation::tests::'])
        self.assertEqual(deletion_selection['max_tests'], 80)
        self.assertIn('paged-deletion-review', tests['selection_groups'])
        self.assertEqual(deletion_selection,
                         next(target for target in
                              self.policy['groups']['paged-deletion-review']['rust_targets']
                              if target['package'] == 'web-app'
                              and target.get('mode', 'test') == 'test'))
        names = ['files::actions::operation::tests::case', 'files::deletion::case']
        self.assertEqual(CI.selected_tests(names, [], deletion_selection), names)
        with self.assertRaises(ValueError):
            CI.selected_tests(names[:1], [], deletion_selection)
        with self.assertRaises(ValueError):
            CI.selected_tests(names[1:] + [f'files::actions::operation::tests::case_{index}'
                                          for index in range(80)], [], deletion_selection)
        self.assertEqual(compile_only['compile_target'], 'wasm32-unknown-unknown')
        self.assertNotIn('prefixes', compile_only)


if __name__ == '__main__':
    unittest.main()
