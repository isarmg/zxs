"""Select actual parallel implementation modules without replacing older inventories."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'parallel_implementation_targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)
PATHS = {
    'shared-draft-selected-scope': [
        'src/engine/src/engine/fresh_layout/draft_source.rs',
        'src/engine/src/business/batch.rs',
        'src/engine/src/business/batch/working.rs',
        'test/engine/unit/business/file_draft_tests.rs',
        'test/engine/unit/business/shared_draft_copy_tests.rs',
    ],
    'worktree-exact-originals': [
        'src/engine/src/directory_worktree/store.rs',
        'src/engine/src/disk.rs',
        'src/engine/src/disk/admission/upload.rs',
        'src/engine/src/business/workspace.rs',
        'src/engine/src/engine/publication.rs',
        'src/engine/src/engine/publication/directory.rs',
        'src/engine/src/snapshot_worktree.rs',
        'src/engine/src/snapshot_worktree/commit.rs',
        'src/engine/src/snapshot_worktree/edit.rs',
        'src/engine/src/snapshot_worktree/registry/edits.rs',
        'src/engine/src/snapshot_worktree/registry/mounts.rs',
        'src/engine/src/snapshot_worktree/reservations.rs',
        'src/engine/src/snapshot_worktree/source.rs',
        'test/engine/unit/snapshot_worktree/automatic_names.rs',
        'test/engine/unit/snapshot_worktree/complete_resaves.rs',
        'test/engine/unit/snapshot_worktree/explicit_occurrence.rs',
    ],
    'draft-base-retention': [
        'src/engine/src/maintenance.rs',
        'src/engine/src/node_retirement.rs',
        'src/engine/src/node_retirement/working.rs',
        'test/engine/unit/node_retirement_working_tests.rs',
    ],
    'helper-http-owner-budgets': [
        'helper/src/console.rs',
        'helper/src/console/http/job.rs',
        'helper/src/console/http/response_body.rs',
        'helper/src/console/transport.rs',
        'test/helper/unit/console/job_tests.rs',
        'test/helper/unit/console/response_body_tests.rs',
        'test/helper/unit/console/transport_tests.rs',
    ],
    'offline-worker-failures': [
        'src/server/src/offline/service_worker.rs',
        'src/server/src/offline/service_worker/failure.rs',
        'test/server/unit/offline/worker_failure_tests.rs',
    ],
    'foreign-original-material': [
        'src/server/src/offline/foreign_chain.rs',
        'src/server/src/offline/foreign_chain/databases.rs',
        'src/server/src/offline/foreign_chain/identity.rs',
        'src/server/src/offline/foreign_chain/objects.rs',
        'src/server/src/offline/foreign_chain/worker.rs',
        'test/server/unit/offline/foreign_chain_identity_tests.rs',
        'test/server/unit/offline/foreign_chain_tests.rs',
    ],
    'helper-maintenance-replies': [
        'helper/src/maintenance.rs',
        'test/helper/unit/maintenance_tests.rs',
    ],
    'lifecycle-membership-cache': [
        'src/engine/src/engine/history_window/read/lifecycle.rs',
        'src/engine/src/state_projection/lifecycles.rs',
        'src/engine/src/state_projection/lifecycles/membership.rs',
        'test/engine/unit/state_projection/lifecycle_membership.rs',
        'test/engine/unit/engine/file_lifecycle_tests.rs',
    ],
}


class ParallelImplementationSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_changed_sources_and_new_modules_select_their_actual_behavior(self):
        for group, paths in PATHS.items():
            for path in paths:
                with self.subTest(group=group, path=path):
                    plan = CI.select_plan(self.policy, [path], shardable=True)
                    self.assertIn(group, plan['groups'])
                    self.assertIn('test_parallel_implementation_targeted_ci', plan['python_tests'])
        plan = CI.select_plan(self.policy,
                              ['test/ci/test_parallel_implementation_targeted_ci.py'], shardable=True)
        self.assertTrue(set(PATHS) <= set(plan['groups']))

    def test_native_draft_and_worktree_tests_are_not_only_product_checks(self):
        expected = {
            'shared-draft-selected-scope': (3, ['business::batch::shared_draft_copy_tests::']),
            'worktree-exact-originals': (8, ['snapshot_worktree::explicit_occurrence_tests::',
                                            'snapshot_worktree::instance::allocation::tests::']),
            'draft-base-retention': (2, ['node_retirement::working::tests::']),
            'lifecycle-membership-cache': (13, ['state_projection::lifecycles::membership::tests::',
                                               'engine::history_window::read::lifecycle::tests::']),
        }
        for group, (maximum, prefixes) in expected.items():
            runtime, product = self.policy['groups'][group]['rust_targets']
            self.assertEqual(runtime['package'], 'storage-engine')
            self.assertEqual(runtime['kind'], 'lib')
            self.assertEqual(runtime['features'], ['fault-injection'])
            self.assertEqual(runtime['prefixes'], prefixes)
            self.assertEqual(runtime['max_tests'], maximum)
            self.assertEqual(product['mode'], 'check')
            self.assertNotIn('fault-injection', product['features'])

    def test_offline_and_helper_modules_use_their_real_binary_test_targets(self):
        for group, package, name, maximum in [
            ('helper-http-owner-budgets', 'storage-helper', 'zxs-helper', 16),
            ('helper-maintenance-replies', 'storage-helper', 'zxs-helper', 6),
            ('offline-worker-failures', 'storage-server', 'zxs', 2),
            ('foreign-original-material', 'storage-server', 'zxs', 21),
        ]:
            with self.subTest(group=group):
                runtime, product = self.policy['groups'][group]['rust_targets']
                self.assertEqual((runtime['package'], runtime['kind'], runtime['name']),
                                 (package, 'bin', name))
                self.assertEqual(runtime['max_tests'], maximum)
                self.assertEqual(product['mode'], 'check')
                self.assertEqual(product['name'], name)
        helper = self.policy['groups']['helper-http-owner-budgets']['rust_targets'][0]
        self.assertEqual(helper['prefixes'], ['console::transport::tests::',
                                             'console::http::response_body::tests::',
                                             'console::http::job::tests::'])
        foreign = self.policy['groups']['foreign-original-material']['rust_targets'][0]
        self.assertIn('offline::foreign_chain::identity::tests::', foreign['prefixes'])

    def test_previous_worktree_and_backup_scopes_keep_their_own_budgets(self):
        names = self.policy['groups']['worktree-automatic-names']['rust_targets'][0]
        self.assertEqual(names['max_tests'], 6)
        for path, previous in [
            ('test/engine/unit/snapshot_worktree/automatic_names.rs', 'worktree-automatic-names'),
            ('test/engine/unit/snapshot_worktree/complete_resaves.rs', 'worktree-http-transport'),
            ('src/engine/src/snapshot_worktree/edit.rs', 'fresh-managed-core'),
            ('test/server/unit/offline/foreign_chain_tests.rs', 'offline-chain-slots'),
        ]:
            self.assertIn(previous,
                          CI.select_plan(self.policy, [path], shardable=True)['groups'])
        for path in ('src/engine/src/engine/backup_preparation/native_control/databases/'
                     'scoped/encoding_memo.rs', 'test/engine/unit/backup_scope_encoding_memo_tests.rs'):
            self.assertIn('backup-scoped-encoding-memo',
                          CI.select_plan(self.policy, [path], shardable=True)['groups'])
        backup = self.policy['groups']['backup-scoped-encoding-memo']['rust_targets']
        self.assertIn('engine::backup_preparation::native_control::databases::scoped::'
                      'encoding_memo::tests::', backup[0]['prefixes'])
        transaction = next(target for target in backup
                           if target.get('name') == 'recursive_backup_paths')
        self.assertTrue(transaction['isolate'])
        self.assertEqual(transaction['timeout_seconds'], 900)
        archive = next(target for target in backup
                       if target.get('max_tests') == 7 and 'exact' in target)
        self.assertTrue(any(name.endswith('pending_archive_reuse_is_local_and_keeps_fresh_reads_'
                                         'cancellation_and_rollback') for name in archive['exact']))


if __name__ == '__main__':
    unittest.main()
