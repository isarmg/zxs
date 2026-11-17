"""Regression coverage for declared selection paths; never substitute it for executing a shard."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('current_branch_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class CurrentBranchSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_changed_inputs_select_held_budget_helper_and_admission_tests(self):
        for group, files in {
            'setup-and-browser-secret-ownership': ['src/web/src/api/session.rs', 'src/server/src/setup_recovery.rs',
                                                   'test/tools/browser-smoke/runner/error_feedback.js'],
            'normal-startup-tail-join': ['src/server/src/startup_tail.rs', 'test/server/unit/smb_rpc/startup_tests.rs'],
            'ordinary-exact-base-range': ['src/engine/src/container_storage/ordinary_preparation/draft/base_range.rs'],
            'original-ordinary-serving-selection': ['test/engine/unit/backup_ordinary_selection_barrier_tests.rs',
                                                   'test/engine/unit/container_storage/backup_serving_paths_tests.rs'],
            'original-cleanup-readback': ['test/engine/unit/cleanup_readback_tests.rs'],
            'offline-disjoint-held-roots': ['src/server/src/offline/managed_layout/physical.rs'],
            'held-helper-installation-files': [
                'helper/src/deployment/install.rs', 'helper/src/private_io/configuration_files.rs',
                'test/helper/unit/installation/vfs_records.rs', 'helper/src/private_io/fixed_program.rs',
            ],
            'selected-scope-and-physical-images': [
                'src/engine/src/engine/index/system_changes/prepared.rs',
                'src/engine/src/container_storage/file_database/physical.rs',
                'test/engine/unit/container_storage/backup_original_inventory_tests.rs',
                'test/engine/unit/container_storage/backup_target_names.rs',
                'src/engine/src/database_install/existing.rs',
                'src/engine/src/engine/backup_preparation/file_epochs.rs',
                'src/engine/src/error.rs',
            ],
            'nested-ordinary-and-smb-birth': [
                'test/server/unit/smb_rpc/native_readonly_birth.rs',
                'test/engine/unit/snapshot_worktree/mounted_path_budget.rs',
                'src/engine/src/business/file_draft/trace.rs',
                'test/engine/unit/snapshot_worktree/draft_gateway/first_preflight.rs',
            ],
            'helper-existing-host-boundaries': [
                'src/server/src/startup_admission.rs', 'test/helper/unit/console/runtime_tests.rs',
                'test/helper/unit/console/configuration_recovery_tests.rs',
            ],
            'foreign-original-material': ['src/server/src/offline/foreign_chain/delivery.rs',
                                         'src/server/src/offline/service_worker.rs'],
            'automatic-management-observation': [
                'src/web/src/files/commit_mode.rs', 'src/server/src/engine_backend/application/file_reads.rs',
                'src/web/src/files/commit_mode/warning.rs', 'src/web/src/files/automatic_observation/calendar.rs',
                'test/server/unit/http_automatic_observation.rs', 'src/contract/src/file_draft/automatic.rs',
            ],
            'original-holder-recovery': [
                'test/engine/unit/backup_scope_file_refs.rs', 'test/helper/unit/samba_password_state.rs',
                'test/server/integration/phase1_save_tags/worktree_commit_recovery.rs',
                'test/engine/unit/engine/scope_genesis_tests.rs', 'src/engine/src/maintenance/live_pins.rs',
            ],
            'backup-held-entry-budget': [
                'src/engine/src/engine/backup_preparation/batch.rs',
                'src/engine/src/engine/backup_preparation/capacity.rs',
                'src/engine/src/engine/backup_preparation/execution.rs',
                'src/engine/src/history_package/preparation.rs',
                'test/engine/unit/backup_copy_capacity_tests.rs',
            ],
            'same-payload-repair-originals': ['helper/src/installation/repair.rs',
                                              'src/platform/src/publication/bundle.rs'],
            'real-browser-first-commit-delivery': ['test/tools/browser-smoke/runner/first_commit_tabs.rs',
                                                  'test/tools/browser-smoke/runner/first_commit_tabs.js',
                                                  'test/tools/browser-smoke/runner/automatic_files/fixture.rs'],
            'helper-stop-handoff': ['helper/src/console/shutdown.rs', 'helper/src/console/http.rs'],
            'cold-lifecycle-receipt-rows': ['test/engine/unit/path_changes_tests.rs'],
            'permanent-removal-exact-leaves': ['src/engine/src/disk/version_material.rs'],
            'worktree-exact-originals': ['src/engine/src/snapshot_worktree/public/namespace.rs'],
            'native-first-manual-save': ['src/web/src/lib.rs', 'src/web/src/browser_lock.rs',
                                         'src/web/src/files/unborn/recovery.rs'],
            'browser-identity-generation': ['src/web/src/browser_lock.rs'],
        }.items():
            for path in files:
                with self.subTest(group=group, path=path):
                    self.assertIn(group, CI.select_plan(self.policy, [path], shardable=True)['groups'])

    def test_native_first_commit_and_offline_storage_inventories_are_not_compile_only(self):
        for group, package, kind, case in [
            ('native-first-manual-save', 'web-app', 'lib',
             'files::unborn::tests::cleanup_completion_requires_the_same_owner_epoch_and_exact_settled_record'),
            ('fresh-managed-core', 'storage-server', 'bin',
             'offline::initialization::storage::tests::first_storage_execution_budget_precedes_host_access_and_retained_intent'),
        ]:
            target = next(t for t in self.policy['groups'][group]['rust_targets']
                          if t['package'] == package and t.get('kind') == kind and case in t.get('exact', []))
            self.assertEqual(len(target['exact']), target['max_tests'])
        backup = self.policy['groups']['backup-held-entry-budget']['rust_targets']
        self.assertTrue(any(t.get('name') == 'recursive_backup_paths' and t.get('isolate')
                            and t.get('timeout_seconds') == 900 for t in backup))


if __name__ == '__main__':
    unittest.main()
