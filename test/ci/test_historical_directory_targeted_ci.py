"""Check historical branch routing and keep its native budgets independently enforced."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'historical_directory_targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class HistoricalDirectoryTargetedCi(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())
        self.group = self.policy['groups']['historical-directory-runtime']

    def test_specialized_regression_runs_runtime_and_policy_checks_within_eight_modules(self):
        plan = CI.select_plan(self.policy, ['test/ci/test_historical_directory_targeted_ci.py'])
        self.assertEqual(plan['groups'], ['ci-selection', 'historical-directory-runtime'])
        self.assertIn('test_targeted_ci', plan['python_tests'])
        self.assertIn('test_historical_directory_targeted_ci', plan['python_tests'])
        self.assertEqual(len(plan['python_tests']), 8)
        self.assertEqual(len(plan['rust_targets']), 4)
        self.assertTrue(all(target.get('mode', 'test') == 'test'
                            for target in plan['rust_targets']))

    def test_historical_admission_publication_and_http_paths_select_runtime_behavior(self):
        for path in [
            'src/engine/src/business/directory_git/branches.rs',
            'src/engine/src/engine/disk_registry/current/directory_branches.rs',
            'src/engine/src/engine/disk_registry/current/directory_staging.rs',
            'src/engine/src/engine/disk_registry/current/file_tags/publication.rs',
            'src/engine/src/persist/directory_branch.rs',
            'src/server/src/backend_disk_directory.rs',
            'src/server/src/engine_backend/file_manager/activity.rs',
            'src/server/src/http_disk_directory.rs',
            'test/engine/integration/directory_git_index/historical_branch.rs',
            'test/engine/unit/engine/current_directory_branch_proof_tests.rs',
            'test/server/integration/http_engine/historical_directory_branch.rs',
        ]:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('historical-directory-runtime', plan['groups'])
                self.assertIn('test_historical_directory_targeted_ci', plan['python_tests'])
                self.assertLessEqual(len(plan['rust_targets']), 16)
                selected = [target for target in plan['rust_targets']
                            if 'historical-directory-runtime' in target['selection_groups']]
                self.assertEqual(len(selected), 4)

    def test_server_authorization_fixtures_preserve_the_complete_server_selection(self):
        group = self.policy['groups']['server-authorization']
        self.assertEqual(group['rust_targets'], [self.group['rust_targets'][2]])
        self.assertEqual(group['rust_targets'][0]['max_tests'], 7)
        for path in [
            'test/server/unit/authorized_backend.rs',
            'test/server/unit/engine_backend/revocation_tests.rs',
        ]:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('server-authorization', plan['groups'])
                self.assertNotIn('historical-directory-runtime', plan['groups'])
                self.assertIn('test_historical_directory_targeted_ci', plan['python_tests'])
                selected = [target for target in plan['rust_targets']
                            if 'server-authorization' in target['selection_groups']]
                self.assertEqual(len(selected), 1)
                self.assertEqual(selected[0]['package'], 'storage-server')

    def test_note_constructors_select_existing_smb_and_retained_metadata_behavior(self):
        group = self.policy['groups']['note-target-consumers']
        unit, integration = group['rust_targets']
        self.assertEqual(unit['exact'], [
            'engine_backend::authorization_tests::retained_metadata::'
            'retained_metadata_d3_preserves_current_authorization_for_edits_and_receipts',
            'smb_rpc::tests::default_share_tests::phase1_new_file_tests::'
            'phase1_primary_source_read_to_new_target_keeps_independent_history',
        ])
        self.assertEqual(integration['exact'], [
            'public_smb_bridge_read_to_new_file_keeps_independent_history_after_restart',
            'soft_deletion::'
            'public_smb_soft_delete_revokes_cached_reads_and_pages_and_restores_exact_file',
            'soft_deletion::'
            'public_backend_soft_delete_preserves_frontier_and_freezes_only_deleted_identity',
        ])
        self.assertEqual([unit['max_tests'], integration['max_tests']], [2, 3])
        self.assertEqual(integration['name'], 'phase1_smb_rpc')
        for target in group['rust_targets']:
            self.assertEqual(target['features'], ['core'])
            self.assertEqual(target['timeout_seconds'], 900)
            self.assertEqual(target.get('mode', 'test'), 'test')
        for path in [
            'test/server/integration/phase1_smb_rpc.rs',
            'test/server/integration/phase1_smb_rpc/soft_deletion.rs',
            'test/server/unit/engine_backend/retained_metadata_tests.rs',
            'test/server/unit/smb_rpc/phase1_new_file_tests.rs',
        ]:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('note-target-consumers', plan['groups'])
                self.assertEqual(len(plan['rust_targets']), 2)

    def test_note_target_batch_has_complete_mapping_within_sixteen_builds(self):
        plan = CI.select_plan(self.policy, [
            '.github/ci/targeted-tests.json',
            'TASKS.md',
            'src/server/src/backend_note.rs',
            'src/server/src/engine_backend.rs',
            'src/server/src/http_note.rs',
            'src/server/src/lib.rs',
            'test/server/integration/http_engine/note_boundary.rs',
            'test/server/integration/phase1_smb_rpc.rs',
            'test/server/integration/phase1_smb_rpc/soft_deletion.rs',
            'test/server/unit/authorized_backend.rs',
            'test/server/unit/engine_backend/retained_metadata_tests.rs',
            'test/server/unit/engine_backend/revocation_tests.rs',
            'test/server/unit/http_note.rs',
            'test/server/unit/smb_rpc/phase1_new_file_tests.rs',
        ])
        self.assertEqual(len(plan['changed_paths']), 14)
        self.assertEqual(len(plan['rust_targets']), 16)
        self.assertEqual(len(plan['python_tests']), 8)
        self.assertIn('portable-path-consumers', plan['groups'])
        self.assertIn('paged-deletion-review', plan['groups'])
        self.assertIn('note-http-boundary', plan['groups'])
        self.assertIn('server-authorization', plan['groups'])
        self.assertIn('note-target-consumers', plan['groups'])

    def test_native_targets_preserve_core_features_exact_scope_and_each_budget(self):
        targets = self.group['rust_targets']
        self.assertEqual([(target['package'], target.get('kind', 'lib'),
                           target.get('name'), target['features'], target['max_tests'])
                          for target in targets], [
            ('storage-engine', 'lib', None, [], 6),
            ('storage-engine', 'test', 'directory_git_index', [], 5),
            ('storage-server', 'lib', None, ['core'], 7),
            ('storage-server', 'test', 'http_engine', ['core'], 5),
        ])
        self.assertEqual(targets[1]['prefixes'], ['historical_branch::'])
        self.assertEqual(targets[3]['prefixes'],
                         ['historical_directory_branch::', 'directory_branch_source::'])
        self.assertEqual(len(targets[0]['exact']), 6)
        self.assertTrue(all(target['timeout_seconds'] == 900 for target in targets))
        for target in targets:
            command = CI.cargo_command(target, '/build')
            self.assertEqual(command[1], 'test')
            self.assertIn('--no-run', command)
            self.assertNotIn('--workspace', command)

    def test_contract_and_runtime_batches_still_obey_the_sixteen_build_ceiling(self):
        with self.assertRaises(ValueError):
            CI.select_plan(self.policy, [
                'src/contract/src/directory_git/historical_branch.rs',
                'src/engine/src/business/directory.rs',
                'src/engine/src/business/workspace.rs',
                'test/ci/test_layout_targeted_ci.py',
            ])


if __name__ == '__main__':
    unittest.main()
