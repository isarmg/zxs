"""Keep the actual terminal Backup transaction and each source boundary selected."""
import json
from pathlib import Path
import unittest

from test_targeted_ci import CI, assert_path_group_extension, origin_selections

ROOT = Path(__file__).resolve().parents[2]
GROUP = 'terminal-backup-source-layout'
EXACT = ('backup_preparation::transaction_serving::'
         'same_backup_switch_serves_and_replays_normal_successor_publications')
PATHS = [
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/target_locator/store.rs',
    'test/engine/unit/backup_target_locator_tests.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/first_publication.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/configuration.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/serving.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/serving/reads.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/published_scopes.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/common_databases.rs',
    'src/engine/src/container_storage.rs',
    'src/engine/src/container_storage/file_database/install.rs',
    'src/engine/src/container_storage/ordinary_preparation.rs',
    'src/engine/src/container_storage/ordinary_preparation/backup.rs',
    'src/engine/src/container_storage/ordinary_preparation/backup/originals.rs',
    'src/engine/src/container_storage/ordinary_preparation/backup/serving.rs',
    'src/engine/src/engine/backup_preparation.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/fence.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/ordinary_serving.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/recovery.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/runtime.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/serving_activation.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/serving_activation/original_objects.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/serving_activation/recovery.rs',
    'src/engine/src/engine/backup_preparation/target_resources.rs',
    'src/engine/src/engine/fresh_layout.rs',
    'src/engine/src/engine/fresh_layout/containers.rs',
    'src/engine/src/engine/membership_transition.rs',
    'src/engine/src/engine/membership_transition/backup.rs',
    'src/engine/src/engine/membership_transition/backup/generation/first_candidate.rs',
    'src/engine/src/engine/membership_transition/backup/registry.rs',
    'src/engine/src/engine/membership_transition/backup/registry/first_publication/evidence.rs',
    'src/engine/src/engine/membership_transition/backup/registry/payloads.rs',
    'src/engine/src/engine/membership_transition/backup/registry/recovery.rs',
    'src/engine/src/engine/membership_transition/backup/registry/roots.rs',
    'src/engine/src/engine/membership_transition/backup/registry/roots/layout_source.rs',
    'src/engine/src/engine/membership_transition/backup/registry/roots/layouts.rs',
    'src/engine/src/engine/membership_transition/pending/activation/backup.rs',
    'src/engine/src/engine/membership_transition/pending/activation/backup/startup.rs',
    'src/engine/src/engine/membership_transition/pending/activation/backup/startup/native.rs',
    'src/engine/src/engine/publication.rs',
    'src/engine/src/journal.rs',
    'src/engine/src/journal/version_publication.rs',
    'src/engine/src/persist/layout.rs',
    'test/engine/integration/backup_transaction_serving.rs',
    'test/engine/unit/backup_original_scope_encoding_tests.rs',
    'test/engine/unit/backup_serving_cost_tests.rs',
    'docs/validation/terminal-backup-source-layout.md',
    'test/ci/test_terminal_backup_source_ci.py',
]


class TerminalBackupSourceSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_every_actual_source_and_fixture_selects_the_full_transaction(self):
        for path in PATHS:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                self.assertIn(GROUP, plan['groups'])
                self.assertIn('test_terminal_backup_source_ci', plan['python_tests'])
                self.assertCountEqual(origin_selections(plan, GROUP),
                                      self.policy['groups'][GROUP]['rust_targets'])
        assert_path_group_extension(self, self.policy, PATHS, None, [GROUP])

    def test_exact_default_and_fault_transaction_keep_the_original_900_seconds(self):
        default, faults, product, lean = self.policy['groups'][GROUP]['rust_targets']
        for target in (default, faults):
            self.assertEqual(target['exact'], [EXACT])
            self.assertEqual((target['name'], target['max_tests'], target['timeout_seconds']),
                             ('recursive_backup_paths', 1, 900))
            self.assertTrue(target['default_features'])
            self.assertTrue(target['isolate'])
            self.assertNotIn('prefixes', target)
            self.assertNotIn('whole', target)
        self.assertEqual(default['features'], [])
        self.assertEqual(faults['features'], ['fault-injection'])
        self.assertEqual([product['mode'], lean['mode']], ['check', 'check'])
        self.assertEqual([product['default_features'], lean['default_features']], [True, False])

    def test_shared_sources_keep_the_original_independent_selections(self):
        for path, groups in {
            'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/target_locator/store.rs':
                ['held-directory-layout'],
            'test/engine/unit/backup_target_locator_tests.rs': ['fresh-managed-core'],
            'src/engine/src/journal.rs': ['backup-scoped-encoding-memo',
                                        'version-publication-location', 'held-directory-layout'],
            'src/engine/src/container_storage/file_database/install.rs':
                ['file-material-copy-reuse', 'current-name-layout-consumers'],
            'src/engine/src/persist/layout.rs': ['checkpoint-layout', 'held-directory-layout',
                                               'projection-cache-layout'],
            'test/engine/integration/backup_transaction_serving.rs':
                ['backup-database-snapshot', 'backup-entity-entry-budget'],
        }.items():
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                for group in groups:
                    self.assertIn(group, plan['groups'])
                    self.assertCountEqual(origin_selections(plan, group),
                                          self.policy['groups'][group]['rust_targets'])

    def test_locator_address_selects_actual_inventory_guards_and_full_terminal(self):
        group = 'backup-target-locator-address'
        paths = [
            'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/target_locator/store.rs',
            'test/engine/unit/backup_target_locator_tests.rs',
            'test/ci/test_terminal_backup_source_ci.py',
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                self.assertIn(group, plan['groups'])
                self.assertIn(GROUP, plan['groups'])
                for selected in (group, GROUP):
                    self.assertCountEqual(origin_selections(plan, selected),
                                          self.policy['groups'][selected]['rust_targets'])
        target, = self.policy['groups'][group]['rust_targets']
        prefix = ('engine::backup_preparation::native_control::databases::scoped::'
                  'cutover::registration::target_locator::store::tests::')
        names = [
            'observing_partial_locator_never_creates_missing_native_files',
            'unowned_matching_native_namespace_is_not_adopted',
            'selected_complete_locator_loss_is_detected_without_repair',
            'data_only_target_inventory_rejects_an_active_metadata_journal',
            'original_locator_selection_cannot_be_relabelled',
            'copied_locator_selection_cannot_rebind_a_second_directory',
            'generated_locator_paths_cannot_escape_fixed_native_namespaces',
            'changed_native_file_is_preserved_and_rejected_by_readback',
        ]
        self.assertEqual(target['exact'], [prefix + name for name in names])
        self.assertEqual((target['kind'], target['max_tests'], target['timeout_seconds']),
                         ('lib', 8, 120))
        self.assertTrue(target['default_features'])
        self.assertEqual(target['features'], [])
        self.assertNotIn('prefixes', target)
        self.assertNotIn('whole', target)
        assert_path_group_extension(self, self.policy, paths, None, [group, GROUP])


if __name__ == '__main__':
    unittest.main()
