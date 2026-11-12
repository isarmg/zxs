"""Exercise change selection and fail-closed runtime evidence with synthetic inputs."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class TargetedSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_directory_branch_web_retirement_preserves_shared_recovery_and_bounded_ci(self):
        paths = [
            'docs/design/directory-branch-web-retirement.md',
            'docs/tools/browser-smoke.md',
            'src/web/src/files/actions/mod.rs',
            'src/web/src/files/actions/mutation.rs',
            'src/web/src/files/actions/operation.rs',
            'src/web/src/files/branch.rs',
            'src/web/src/files/detail/directory.rs',
            'src/web/src/files/inline.rs',
            'src/web/src/files/mkdir.rs',
            'src/web/src/files/mkdir/storage.rs',
            'src/web/src/files/mkdir/storage/directory.rs',
            'src/web/src/files/model.rs',
            'src/web/src/files/staging.rs',
            'src/web/src/files/view.rs',
            'src/web/src/files/view/directory_preview.rs',
            'test/tools/browser-smoke/runner/backup_browse.js',
            'test/tools/browser-smoke/runner/feedback_lifecycle.rs',
            'test/tools/browser-smoke/runner/main.rs',
            'test/tools/browser-smoke/runner/metadata.rs',
            'test/tools/browser-smoke/runner/naming.rs',
            'test/tools/browser-smoke/unit/backup_browse_probe_test.mjs',
            'test/tools/browser-smoke/unit/feedback_lifecycle_probe_test.mjs',
            'test/tools/browser-smoke/unit/main_tests.rs',
            'test/web/unit/files/actions/mutation/tests.rs',
            'test/web/unit/files/branch/tests.rs',
            'test/web/unit/files/directory_branch_recovery_tests.rs',
            'test/web/unit/files/directory_rename_recovery_tests.rs',
            'test/web/unit/files/inline/tests.rs',
            'test/web/unit/files/model/tests.rs',
            'test/web/unit/files/namespace_recovery_tests.rs',
            'test/web/unit/files/staging_tests.rs',
            '.github/ci/targeted-tests.json',
            'test/ci/test_targeted_ci.py',
            'test/ci/test_directory_rename_targeted_ci.py',
        ]
        with self.assertRaises(ValueError):
            CI.select_plan(self.policy, paths)
        plan = CI.select_plan(self.policy, paths, shardable=True)
        self.assertEqual(plan['groups'], [
            'ci-selection', 'container-tags-browser', 'container-tags-web',
            'directory-rename-recovery', 'historical-directory-web',
            'legacy-directory-branch-web-retirement',
            'legacy-directory-staging-web-retirement', 'paged-deletion-review',
            'web-directory-history-retirement', 'web-recovery-principals',
        ])
        self.assertEqual(len(plan['rust_targets']), 8)
        self.assertEqual(plan['python_tests'],
                         sorted(set(self.policy['groups']['ci-selection']['python_tests'])
                                | set(self.policy['groups']['container-tags-browser']['python_tests'])))
        self.assertEqual(len(plan['python_tests']), 10)
        group = self.policy['groups']['legacy-directory-branch-web-retirement']
        self.assertEqual(group['rust_targets'][0]['prefixes'], [
            'files::mkdir::storage::tests::', 'files::branch::tests::',
            'files::inline::tests::', 'files::model::tests::', 'files::staging::tests::',
            'files::actions::mutation::tests::', 'files::actions::operation::tests::',
        ])
        self.assertEqual(group['rust_targets'][0]['max_tests'], 144)
        namespace = CI.select_plan(self.policy,
                                  ['test/web/unit/files/namespace_recovery_tests.rs'])
        self.assertEqual(namespace['groups'], ['legacy-directory-branch-web-retirement'])
        self.assertEqual(len(namespace['rust_targets']), 4)
        self.assertEqual(namespace['python_tests'], [])
        rename = self.policy['groups']['directory-rename-recovery']
        self.assertNotIn('files::mkdir::storage::rename_tests::',
                         rename['rust_targets'][0]['prefixes'])
        self.assertIn('files::mkdir::storage::tests::',
                      rename['rust_targets'][0]['prefixes'])

    def test_readmission_fixture_retains_all_working_directory_consumers(self):
        path = 'test/engine/unit/engine/disk_readmission_tests.rs'
        required = [
            'checkpoint-layout', 'held-directory-layout',
            'ordinary-conversion-name-budget', 'paged-deletion-review',
            'projection-cache-layout', 'typed-name-consumers',
            'working-directory-prerequisite',
        ]
        plan = CI.select_plan(self.policy, [path])
        self.assertEqual(plan['groups'], required)
        self.assertEqual(len(plan['rust_targets']), 13)
        self.assertEqual(plan['python_tests'], [])
        held = self.policy['groups']['held-directory-layout']['rust_targets'][0]
        self.assertEqual(held['max_tests'], 190)
        self.assertEqual(held.get('timeout_seconds', 300), 300)
        self.assertIn('engine::membership_transition::readmission::', held['prefixes'])
        full = CI.select_plan(self.policy, [
            path, 'test/engine/unit/disk/working_directory_tests.rs',
            'test/ci/test_directory_rename_targeted_ci.py',
            CI.PLAN_PATH, 'test/ci/test_targeted_ci.py',
        ])
        self.assertEqual(full['groups'],
                         sorted(required + ['ci-selection', 'directory-rename-recovery']))
        self.assertEqual(len(full['rust_targets']), 13)
        self.assertEqual(full['python_tests'],
                         sorted(self.policy['groups']['ci-selection']['python_tests']))
        self.assertEqual(len(full['python_tests']), 7)

    def test_publication_location_refactor_keeps_proof_and_live_layout_selections(self):
        paths = [
            'src/engine/src/journal.rs', 'src/engine/src/persist/layout.rs',
            'src/engine/src/engine/provenance.rs',
            'src/engine/src/journal/version_publication.rs',
            'test/engine/unit/journal/version_publication_fixture.rs',
            'test/engine/unit/journal/version_publication_tests.rs',
            'test/engine/unit/engine/provenance_publication_tests.rs',
            CI.PLAN_PATH, 'test/ci/test_targeted_ci.py',
        ]
        plan = CI.select_plan(self.policy, paths)
        self.assertEqual(plan['groups'], [
            'checkpoint-layout', 'ci-selection', 'held-directory-layout',
            'projection-cache-layout', 'version-publication-location',
        ])
        self.assertEqual(len(plan['rust_targets']), 4)
        self.assertEqual(plan['python_tests'],
                         sorted(self.policy['groups']['ci-selection']['python_tests']))
        self.assertEqual(len(plan['python_tests']), 7)
        engine = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-engine'
                      and target.get('mode', 'test') == 'test'
                      and not target.get('features'))
        proof = next(selection for selection in engine['selections']
                     if 'journal::version_publication::tests::'
                     in selection.get('prefixes', []))
        self.assertEqual(proof['prefixes'], [
            'journal::version_publication::tests::',
            'engine::provenance::publication_tests::',
            'journal::proof_decoding_tests::', 'engine::reads::tests::',
            'engine::integrity_scrub::tests::',
        ])
        self.assertEqual(proof['max_tests'], 144)
        self.assertEqual(proof['exact'], [
            'disk::export_tests::offline_tombstones_bind_retained_version_metadata_and_directory',
            'disk::export_tests::restored_tombstones_need_no_source_payload_proofs',
        ])
        product = next(target for target in plan['rust_targets']
                       if target.get('mode') == 'check')
        self.assertEqual(product['package'], 'storage-engine')
        self.assertFalse(product['default_features'])

    def test_publication_location_sources_have_no_unmapped_leaf(self):
        paths = [
            'src/engine/src/journal/version_publication.rs',
            'test/engine/unit/journal/version_publication_fixture.rs',
            'test/engine/unit/journal/version_publication_tests.rs',
            'test/engine/unit/engine/provenance_publication_tests.rs',
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'], ['version-publication-location'])
                self.assertEqual(len(plan['rust_targets']), 2)
                self.assertEqual(plan['python_tests'], [])
        for path in ['src/engine/src/journal.rs', 'src/engine/src/engine/provenance.rs']:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'],
                                 ['held-directory-layout', 'version-publication-location'])
                self.assertEqual(len(plan['rust_targets']), 3)

    def test_legacy_directory_staging_retirement_keeps_live_groups_with_eight_builds(self):
        paths = [
            'src/web/src/files/staging.rs', 'src/web/src/files/actions/mutation.rs',
            'src/web/src/files/actions/mod.rs', 'src/web/src/files/detail/directory.rs',
            'src/web/src/files/detail.rs', 'test/web/unit/files/staging_tests.rs',
            'src/web/src/files/batch/actions.rs', 'src/web/src/files/batch/view.rs',
            'src/web/src/files/batch/mod.rs', 'src/web/src/files/actions/operation.rs',
            'test/web/unit/files/batch/request/tests.rs',
            'test/tools/browser-smoke/runner/fixture.rs',
            'test/tools/browser-smoke/runner/naming.rs',
            'test/tools/browser-smoke/runner/main.rs',
            'test/tools/browser-smoke/runner/metadata.rs',
            'test/tools/browser-smoke/unit/fixture_tests.rs',
            'test/tools/browser-smoke/unit/main_tests.rs',
            'test/tools/browser-smoke/runner/directory_git.rs',
            'test/tools/browser-smoke/runner/directory_git/children.rs',
            'test/tools/browser-smoke/runner/directory_git/fixture.rs',
            'test/tools/browser-smoke/unit/directory_git_probe_test.mjs',
            'test/tools/browser-smoke/unit/directory_git_fixture_tests.rs',
            'test/ci/test_directory_git_probe.py',
            'docs/design/directory-staging-web-retirement.md',
            'docs/tools/browser-smoke.md',
            CI.PLAN_PATH, 'test/ci/test_targeted_ci.py',
        ]
        with self.assertRaises(ValueError):
            CI.select_plan(self.policy, paths)
        plan = CI.select_plan(self.policy, paths, shardable=True)
        self.assertEqual(plan['groups'], [
            'browser-identity-generation', 'browser-smoke-fixture', 'ci-selection',
            'container-tags-browser', 'container-tags-web',
            'directory-rename-recovery',
            'historical-directory-web', 'legacy-directory-branch-web-retirement',
            'legacy-directory-staging-web-retirement',
            'paged-deletion-review', 'web-directory-history-retirement',
        ])
        self.assertEqual(len(plan['rust_targets']), 8)
        self.assertEqual(plan['python_tests'],
                         sorted(set(self.policy['groups']['ci-selection']['python_tests'])
                                | set(self.policy['groups']['container-tags-browser']['python_tests'])))
        self.assertEqual(len(plan['python_tests']), 10)
        self.assertNotIn('working-directory-prerequisite', plan['groups'])
        browser = next(target for target in plan['rust_targets']
                       if target['package'] == 'browser-smoke'
                       and target.get('mode', 'test') == 'test')
        self.assertTrue(any(selection['prefixes'] == ['fixture::tests::', 'tests::']
                            for selection in browser['selections']))
        web = next(target for target in plan['rust_targets']
                   if target['package'] == 'web-app'
                   and target.get('mode', 'test') == 'test')
        self.assertTrue(any('files::batch::request::tests::' in selection.get('prefixes', [])
                            and 'files::actions::operation::tests::'
                            in selection.get('prefixes', [])
                            for selection in web['selections']))

    def test_deleted_directory_browser_writers_select_live_replacements_not_deleted_python(self):
        for path in ['test/tools/browser-smoke/runner/directory_git.rs',
                     'test/tools/browser-smoke/runner/directory_git/children.rs',
                     'test/tools/browser-smoke/runner/directory_git/fixture.rs',
                     'test/tools/browser-smoke/unit/directory_git_probe_test.mjs',
                     'test/tools/browser-smoke/unit/directory_git_fixture_tests.rs',
                     'test/ci/test_directory_git_probe.py']:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'], ['legacy-directory-staging-web-retirement'])
                self.assertEqual(plan['python_tests'], [])
                self.assertEqual(len(plan['rust_targets']), 4)

    def test_working_directory_paths_select_structural_io_and_registry_consumers(self):
        paths = [
            'src/contract/src/working_tree.rs',
            'src/engine/src/working_tree.rs',
            'src/engine/src/working_tree/records.rs',
            'src/engine/src/working_tree/admission.rs',
            'src/engine/src/disk/working_directory.rs',
            'src/engine/src/engine/disk_registry.rs',
            'test/contract/unit/working_tree_tests.rs',
            'test/engine/unit/working_tree_tests.rs',
            'test/engine/unit/disk/working_directory_tests.rs',
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'], ['working-directory-prerequisite'])
                self.assertEqual(plan['python_tests'], [])
                self.assertEqual(len(plan['rust_targets']), 3)
                contract = next(target for target in plan['rust_targets']
                                if target['package'] == 'storage-contract')
                self.assertEqual(contract['prefixes'], ['working_tree::tests::'])
                engine = next(target for target in plan['rust_targets']
                              if target['package'] == 'storage-engine'
                              and target.get('mode', 'test') == 'test')
                self.assertEqual(engine['prefixes'], [
                    'working_tree::records::admission::tests::',
                    'disk::owned::working_directory::tests::',
                    'engine::disk_registry::tests::',
                    'engine::disk_registry::published::tests::',
                ])
                product = next(target for target in plan['rust_targets']
                               if target.get('mode') == 'check')
                self.assertEqual(product['package'], 'storage-engine')
                self.assertFalse(product['default_features'])

    def test_working_directory_prerequisite_keeps_live_mapping_and_thirteen_builds(self):
        paths = [
            'src/contract/src/lib.rs', 'src/contract/src/working_tree.rs',
            'src/engine/src/lib.rs', 'src/engine/src/working_tree.rs',
            'src/engine/src/working_tree/records.rs',
            'src/engine/src/working_tree/admission.rs',
            'src/engine/src/engine/disk_registry.rs',
            'src/engine/src/disk.rs', 'src/engine/src/disk/owned.rs',
            'src/engine/src/disk/working_directory.rs', 'src/engine/src/persist/layout.rs',
            'test/contract/unit/working_tree_tests.rs',
            'test/engine/unit/working_tree_tests.rs',
            'test/engine/unit/disk/working_directory_tests.rs',
            'test/platform/unit/fs_safety/directory-retry.sh',
            CI.PLAN_PATH, 'test/ci/test_targeted_ci.py',
        ]
        plan = CI.select_plan(self.policy, paths)
        self.assertEqual(plan['groups'], [
            'checkpoint-layout', 'ci-selection', 'container-tags-format',
            'held-directory-layout', 'ordinary-conversion-name-budget', 'paged-deletion-review',
            'projection-cache-layout', 'typed-name-consumers', 'working-directory-prerequisite',
        ])
        self.assertEqual(len(plan['rust_targets']), 13)
        self.assertEqual(plan['python_tests'],
                         sorted(self.policy['groups']['ci-selection']['python_tests']))
        self.assertEqual(len(plan['python_tests']), 7)
        central = CI.select_plan(self.policy, ['src/engine/src/lib.rs'])
        self.assertNotIn('working-directory-prerequisite', central['groups'])
        self.assertEqual(central['groups'], [
            'held-directory-layout', 'ordinary-conversion-name-budget',
            'typed-name-consumers',
        ])

    def test_directory_fault_script_changes_execute_the_real_ordinary_parent(self):
        for path in ['test/platform/unit/fs_safety/directory-retry.sh',
                     'test/platform/unit/fs_safety/directory_retry.c']:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'],
                                 ['held-directory-layout', 'working-directory-prerequisite'])
                self.assertEqual(plan['python_tests'], [])
                self.assertEqual(len(plan['rust_targets']), 4)
                engine = next(target for target in plan['rust_targets']
                              if target['package'] == 'storage-engine'
                              and target.get('mode', 'test') == 'test')
                self.assertEqual(engine['selection_groups'],
                                 ['held-directory-layout', 'working-directory-prerequisite'])
                self.assertTrue(any(
                    'disk::owned::working_directory::tests::' in selection['prefixes']
                    for selection in engine['selections']
                ))

    def test_tooling_change_selects_only_related_python_modules(self):
        plan = CI.select_plan(self.policy, ['.github/scripts/check-targeted-ci.py'])
        self.assertEqual(plan['rust_targets'], [])
        self.assertIn('test_targeted_ci', plan['python_tests'])
        self.assertTrue(plan['unexecuted'])

    def test_snapshot_source_policy_preparation_does_not_execute_future_rust_tests(self):
        plan = CI.select_plan(self.policy, [CI.PLAN_PATH, 'test/ci/test_targeted_ci.py'])
        self.assertEqual(plan['groups'], ['ci-selection'])
        self.assertEqual(plan['rust_targets'], [])
        self.assertEqual(plan['python_tests'],
                         sorted(self.policy['groups']['ci-selection']['python_tests']))
        self.assertEqual(len(plan['python_tests']), 7)
        central = CI.select_plan(self.policy, [
            'src/engine/src/engine.rs', 'src/engine/src/lib.rs',
            'src/server/src/lib.rs', 'src/server/src/backend_methods.rs',
            'src/server/src/engine_backend.rs', 'src/server/src/file_manager.rs',
            'test/server/integration/http_engine.rs',
        ], shardable=True)
        self.assertNotIn('directory-snapshot-source-preview', central['groups'])
        self.assertIn('historical-directory-runtime', central['groups'])
        self.assertIn('version-metadata-read', central['groups'])

    def test_snapshot_source_paths_select_real_engine_and_http_preview_modules(self):
        paths = [
            'src/engine/src/engine/directory_snapshot_source.rs',
            'test/engine/unit/engine/directory_snapshot_source_tests.rs',
            'src/server/src/http_directory_snapshot_source.rs',
            'src/server/src/backend_directory_snapshot_source.rs',
            'src/server/src/engine_backend/directory_snapshot_source.rs',
            'test/server/integration/http_engine/directory_snapshot_source.rs',
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'], ['directory-snapshot-source-preview'])
                self.assertEqual(plan['python_tests'], [])
                self.assertEqual(len(plan['rust_targets']), 3)
                engine = next(target for target in plan['rust_targets']
                              if target['package'] == 'storage-engine'
                              and target.get('mode', 'test') == 'test')
                self.assertEqual(engine['prefixes'],
                                 ['engine::directory_snapshot_source::tests::'])
                http = next(target for target in plan['rust_targets']
                            if target['package'] == 'storage-server')
                self.assertEqual(http['kind'], 'test')
                self.assertEqual(http['name'], 'http_engine')
                self.assertEqual(http['features'], ['core'])
                self.assertEqual(http['prefixes'], ['directory_snapshot_source::'])
                product = next(target for target in plan['rust_targets']
                               if target.get('mode') == 'check')
                self.assertEqual(product['package'], 'storage-engine')
                self.assertFalse(product['default_features'])

    def test_backup_recovery_paths_select_their_behavior_without_adjacent_actions(self):
        paths = ['src/web/src/admin/backup_transaction/recovery.rs',
                 'test/web/unit/admin/backup_transaction_recovery.rs']
        for path in paths:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'],
                                 ['browser-identity-generation', 'web-session-principals'])
                self.assertEqual(len(plan['rust_targets']), 2)
                self.assertEqual(plan['python_tests'], [])
                native = next(target for target in plan['rust_targets']
                              if target.get('mode', 'test') == 'test')
                self.assertEqual(native['package'], 'web-app')
                self.assertTrue(any('admin::backup_transaction::recovery::tests::'
                                    in selection.get('prefixes', [])
                                    for selection in native['selections']))
                self.assertTrue(all(selection['max_tests'] <= 192
                                    for selection in native['selections']))
                self.assertTrue(any(target.get('compile_target') == 'wasm32-unknown-unknown'
                                    and target.get('mode') == 'check'
                                    for target in plan['rust_targets']))

    def test_layout_and_ci_changes_union_groups_once(self):
        plan = CI.select_plan(self.policy, ['src/engine/src/persist/layout.rs',
                            'src/engine/src/journal.rs', '.github/workflows/ci.yml'])
        self.assertEqual(len(plan['rust_targets']), 4)
        self.assertEqual(len(plan['groups']), 5)
        self.assertIn('test_development_ci', plan['python_tests'])

    def test_container_history_dag_routes_both_layout_consumers_and_boundaries(self):
        paths = [
            'src/web/src/files/history/ingress.rs',
            'src/web/src/files/version_tree.rs',
            'test/web/unit/files/history/window_tests.rs',
            'test/web/unit/files/version_tree/tests.rs',
            'test/web/unit/files/actions/history_tests.rs',
            'docs/design/container-history-dag-admission.md',
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('container-history-dag-admission', plan['groups'])
                web = next(target for target in plan['rust_targets']
                           if target['package'] == 'web-app'
                           and target.get('mode', 'test') == 'test')
                prefixes = {prefix for selection in web.get('selections', [web])
                            for prefix in selection.get('prefixes', [])}
                self.assertTrue({
                    'files::history::ingress::tests::',
                    'files::actions::history::tests::',
                    'files::version_tree::tests::',
                    'files::shared_history::tests::',
                } <= prefixes)
                self.assertTrue(any(target['package'] == 'storage-contract'
                                    and target.get('mode', 'test') == 'test'
                                    for target in plan['rust_targets']))
                checks = [target for target in plan['rust_targets']
                          if target['package'] == 'web-app' and target.get('mode') == 'check']
                self.assertEqual(len(checks), 2)
                self.assertEqual(sum(target.get('compile_target') == 'wasm32-unknown-unknown'
                                     for target in checks), 1)
        self.assertIn('web-directory-history-retirement',
                      CI.select_plan(self.policy, [paths[1]])['groups'])

    def test_container_lifecycle_batches_keep_all_existing_consumers_within_scope(self):
        engine_paths = [
            'src/contract/src/lib.rs', 'src/contract/src/error.rs',
            'src/contract/src/file_lifecycle.rs', 'test/contract/unit/file_lifecycle.rs',
            'src/engine/src/lib.rs', 'src/engine/src/engine.rs',
            'src/engine/src/engine/history_window.rs',
            'src/engine/src/engine/history_window/read.rs',
            'src/engine/src/engine/history_window/read/lifecycle.rs',
            'src/engine/src/file_deletion/retained.rs',
            'test/engine/unit/engine/file_lifecycle_tests.rs',
            'test/engine/integration/file_history_copy.rs',
            'test/engine/integration/file_history_copy/lifecycle_page.rs',
        ]
        ui_paths = [
            'src/server/src/backend_methods.rs', 'src/server/src/engine_backend.rs',
            'src/server/src/file_manager.rs', 'src/server/src/lib.rs',
            'src/server/src/http_file_lifecycle.rs',
            'test/server/integration/phase1_save_tags.rs',
            'test/server/integration/phase1_save_tags/file_lifecycle.rs',
            'src/web/src/files/mod.rs', 'src/web/src/files/detail.rs',
            'src/web/src/files/lifecycle.rs', 'test/web/unit/files/lifecycle_tests.rs',
        ]
        for paths, required in [
            (engine_paths, {'container-lifecycle-contract', 'container-lifecycle-engine',
                            'held-directory-layout', 'historical-directory-runtime',
                            'history-visible-context', 'typed-name-consumers',
                            'ordinary-conversion-name-budget', 'paged-deletion-review'}),
            (ui_paths, {'container-lifecycle-http', 'container-lifecycle-web',
                        'paged-deletion-review', 'note-http-boundary',
                        'version-metadata-read', 'historical-directory-runtime',
                        'directory-rename-http-retirement', 'web-directory-history-retirement',
                        'legacy-directory-staging-web-retirement'}),
        ]:
            with self.subTest(paths=paths):
                plan = CI.select_plan(self.policy, paths, shardable=True)
                self.assertTrue(required <= set(plan['groups']))
                for shard in CI.shard_tools().partition(plan):
                    self.assertLessEqual(len(shard['rust_targets']), 16)
                    self.assertLessEqual(len(shard['python_tests']), 8)
        for oversized in [engine_paths + ui_paths,
                          ui_paths + ['TASKS.md'],
                          ui_paths + ['docs/design/container-lifecycle-read.md']]:
            with self.subTest(paths=oversized), self.assertRaises(ValueError):
                CI.select_plan(self.policy, oversized)
        policy_only = CI.select_plan(self.policy, [CI.PLAN_PATH, 'test/ci/test_targeted_ci.py'])
        self.assertEqual(policy_only['groups'], ['ci-selection'])
        self.assertEqual(policy_only['rust_targets'], [])
        engine = self.policy['groups']['container-lifecycle-engine']['rust_targets'][0]
        self.assertIn('container_storage::file_database::tests::', engine['prefixes'])
        self.assertIn('file_deletion::batch::undo::tests::', engine['prefixes'])
        http = self.policy['groups']['container-lifecycle-http']['rust_targets'][0]
        self.assertEqual((http['package'], http['kind'], http['name'], http['features']),
                         ('storage-server', 'test', 'phase1_save_tags', ['core']))

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

    def test_engineering_guidance_paths_select_only_the_real_maintenance_fixture(self):
        paths = [
            'AGENTS.md',
            '禁止防御性编程.md',
            '工程规范.md',
            'test/tools/rust-sources/test_legacy_http_maintenance_integration.py',
            'test/ci/test_engineering_guidance.py',
            'tools/rust-sources/source_layout.py',
            'test/tools/rust-sources/test_source_layout.py',
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'], ['engineering-guidance'])
                self.assertEqual(plan['python_tests'], ['test_engineering_guidance'])
                self.assertEqual(plan['rust_targets'], [])
                self.assertTrue(plan['unexecuted'])

    def test_engineering_guidance_rename_selects_one_group_for_both_paths(self):
        plan = CI.select_plan(self.policy, ['禁止防御性编程.md', '工程规范.md'])
        self.assertEqual(plan['groups'], ['engineering-guidance'])
        self.assertEqual(plan['python_tests'], ['test_engineering_guidance'])
        self.assertEqual(plan['rust_targets'], [])

    def test_engineering_guidance_full_change_keeps_existing_python_budget(self):
        plan = CI.select_plan(self.policy, [
            'AGENTS.md', 'TASKS.md', '禁止防御性编程.md', '工程规范.md',
            'docs/20-engineering.md', 'docs/README.md',
            'docs/adr/0010-smb-write-generations-and-extent-grants.md',
            'docs/architecture/test-layout.md',
            'test/tools/rust-sources/test_legacy_http_maintenance_integration.py',
            '.github/ci/targeted-tests.json', 'test/ci/test_targeted_ci.py',
            'test/ci/test_engineering_guidance.py',
        ])
        self.assertEqual(plan['groups'], ['ci-selection', 'engineering-guidance'])
        self.assertEqual(plan['python_tests'],
                         sorted(self.policy['groups']['ci-selection']['python_tests']
                                + ['test_engineering_guidance']))
        self.assertEqual(len(plan['python_tests']), 8)
        self.assertEqual(plan['rust_targets'], [])

    def test_boundary_watch_changes_select_only_behavioral_metrics_regressions(self):
        for path in ['.github/scripts/check-boundary-metrics.py',
                     '.github/metrics-baseline.json', 'test/ci/test_boundary_metrics.py']:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertEqual(plan['groups'], ['boundary-watch'])
                self.assertEqual(plan['python_tests'], ['test_boundary_metrics'])
                self.assertEqual(plan['rust_targets'], [])

    def test_boundary_watch_migration_keeps_existing_python_budget(self):
        plan = CI.select_plan(self.policy, [
            '.github/scripts/check-boundary-metrics.py', '.github/metrics-baseline.json',
            'test/ci/test_boundary_metrics.py', '.github/ci/targeted-tests.json',
            'test/ci/test_targeted_ci.py',
        ])
        self.assertEqual(plan['groups'], ['boundary-watch', 'ci-selection'])
        self.assertEqual(plan['python_tests'],
                         sorted(self.policy['groups']['ci-selection']['python_tests']
                                + ['test_boundary_metrics']))
        self.assertEqual(len(plan['python_tests']), 8)
        self.assertEqual(plan['rust_targets'], [])

    def test_python_budget_defaults_and_dedicated_maintenance_limit(self):
        plan = CI.select_plan(self.policy, ['工程规范.md', 'TASKS.md'])
        self.assertEqual(len(plan['python_tests']), 8)
        self.assertEqual(plan['python_timeouts']['test_engineering_guidance'], 600)
        for module in plan['python_tests']:
            if module != 'test_engineering_guidance':
                self.assertEqual(plan['python_timeouts'][module], 120)
        plain = CI.select_plan(self.policy, ['TASKS.md'])
        self.assertNotIn('test_engineering_guidance', plain['python_timeouts'])
        self.assertNotIn('engineering-guidance',
                         {item['group'] for item in plain['python_selections']})

    def test_python_budget_rejects_invalid_values_and_unselected_modules(self):
        invalid = [
            [], {'test_engineering_guidance': 0}, {'test_engineering_guidance': 901},
            {'test_engineering_guidance': True}, {'test_engineering_guidance': '600'},
            {'test_engineering_guidance': 1.5}, {'test_not_in_group': 600},
        ]
        for value in invalid:
            policy = copy.deepcopy(self.policy)
            policy['groups']['engineering-guidance']['python_timeouts'] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                CI.select_plan(policy, ['工程规范.md'])

    def test_python_overlapping_groups_keep_sources_and_strictest_budget(self):
        first = {'reason': 'first', 'python_tests': ['test_example'], 'rust_targets': [],
                 'python_timeouts': {'test_example': 600}}
        second = dict(first, reason='second', python_timeouts={'test_example': 300})
        policy = {'groups': {'first': first, 'second': second},
                  'rules': [{'path': 'change', 'groups': ['first', 'second']}],
                  'unexecuted': 'other work'}
        plan = CI.select_plan(policy, ['change'])
        self.assertEqual(plan['python_tests'], ['test_example'])
        self.assertEqual(plan['python_timeouts'], {'test_example': 300})
        self.assertEqual(plan['python_selections'], [
            {'group': 'first', 'module': 'test_example', 'timeout_seconds': 600},
            {'group': 'second', 'module': 'test_example', 'timeout_seconds': 300},
        ])
        del second['python_timeouts']
        self.assertEqual(CI.select_plan(policy, ['change'])['python_timeouts'],
                         {'test_example': 120})

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

    def test_container_save_and_layout_batches_preserve_the_sixteen_build_ceiling(self):
        with self.assertRaises(ValueError):
            CI.select_plan(self.policy, ['src/engine/src/business/directory.rs',
                           'src/engine/src/business/workspace.rs',
                           'test/ci/test_layout_targeted_ci.py'])
        plan = CI.select_plan(self.policy, ['src/engine/src/business/directory.rs',
                            'test/ci/test_layout_targeted_ci.py'])
        self.assertEqual(plan['groups'], ['ci-selection', 'held-directory-layout',
                                         'historical-directory-runtime', 'portable-path-consumers',
                                         'projection-cache-layout'])
        self.assertEqual(len(plan['rust_targets']), 16)
        engine = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-engine' and target.get('kind', 'lib') == 'lib')
        self.assertEqual(engine['selection_groups'],
                         ['held-directory-layout', 'historical-directory-runtime',
                          'portable-path-consumers', 'projection-cache-layout'])
        self.assertEqual([selection['max_tests'] for selection in engine['selections']],
                         [190, 6, 160, 52])

    def test_container_unchanged_save_keeps_tests_product_check_and_overlapping_coverage(self):
        paths = ['src/engine/src/business/workspace.rs', 'test/engine/unit/tests.rs',
                 'test/engine/unit/business/file/pending_regressions.rs',
                 'test/engine/unit/engine/current_branch_archive_tests.rs',
                 'test/engine/integration/branch_alias_operations.rs',
                 '.github/ci/targeted-tests.json', 'test/ci/test_targeted_ci.py']
        plan = CI.select_plan(self.policy, paths, shardable=True)
        self.assertEqual(plan['groups'],
                         ['ci-selection', 'container-tags-engine',
                          'container-unchanged-save', 'portable-path-consumers'])
        self.assertEqual(len(plan['rust_targets']), 20)
        self.assertEqual(len(plan['python_tests']), 7)
        engine = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-engine'
                      and target.get('kind', 'lib') == 'lib'
                      and target.get('mode', 'test') == 'test')
        self.assertEqual(engine['selection_groups'],
                         ['container-tags-engine', 'container-tags-engine',
                          'container-unchanged-save', 'portable-path-consumers'])
        self.assertEqual([selection['max_tests'] for selection in engine['selections']], [32, 1, 24, 160])
        first = engine['selections'][2]
        self.assertEqual(first['prefixes'], ['business::file::pending_regressions::'])
        self.assertEqual(len(first['exact']), 6)
        self.assertIn(
            'engine::disk_registry::current::branch_archives::tests::'
            'current_pending_unchanged_saves_preserve_projection_and_cold_replay',
            first['exact'])
        product = next(target for target in plan['rust_targets']
                       if target['package'] == 'storage-engine'
                       and target.get('mode') == 'check')
        self.assertFalse(product['default_features'])
        self.assertEqual(CI.cargo_command(product, '/build')[1], 'check')

    def test_typed_name_source_and_provenance_paths_are_admitted(self):
        paths = [
            'src/contract/src/repository_name.rs', 'test/contract/unit/repository_name.rs',
            'src/engine/src/business/mod.rs', 'src/engine/src/foreign_import/intent.rs',
            'src/engine/src/foreign_import/mapping.rs', 'src/engine/src/historical_save.rs',
            'src/engine/src/lib.rs', 'test/engine/unit/foreign_import/tests.rs',
            'src/naming/src/lib.rs', 'src/naming/src/name.rs', 'test/naming/unit/names.rs',
            'src/server/src/backend_file_relocation.rs', 'src/server/src/backend_listing.rs',
            'src/server/src/backend_object_move.rs', 'src/server/src/engine_backend/file_manager.rs',
            'src/server/src/http_file_relocation.rs', 'src/server/src/http_object_move.rs',
            'test/server/integration/http_engine/history_alias_operations.rs',
            'test/server/unit/backend_listing.rs']
        for path in paths:
            with self.subTest(path=path):
                self.assertIn('typed-name-consumers',
                              CI.select_plan(self.policy, [path], shardable=True)['groups'])
        for path in ['compliance/external-tests/integrated.json',
                     'docs/THIRD_PARTY_RUST_SOURCES.md']:
            with self.subTest(path=path):
                self.assertEqual(CI.select_plan(self.policy, [path])['rust_targets'], [])
        self.assertIn('portable-path-consumers', CI.select_plan(
            self.policy, ['compliance/rust-sources.json'])['groups'])

    def test_typed_name_overlap_retains_old_groups_with_fifteen_builds(self):
        plan = CI.select_plan(self.policy, [
            'src/naming/src/lib.rs', 'src/engine/src/lib.rs',
            'src/contract/src/repository_name.rs',
            'src/server/src/backend_listing.rs',
            'test/server/integration/http_engine/history_alias_operations.rs',
            '.github/ci/targeted-tests.json'])
        self.assertEqual(plan['groups'], ['ci-selection', 'held-directory-layout',
                                         'ordinary-conversion-name-budget',
                                         'portable-path-consumers', 'typed-name-consumers'])
        self.assertEqual(len(plan['rust_targets']), 15)
        engine = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-engine' and target.get('kind', 'lib') == 'lib')
        self.assertEqual(engine['selection_groups'],
                         ['held-directory-layout', 'ordinary-conversion-name-budget',
                          'portable-path-consumers', 'typed-name-consumers'])
        self.assertEqual([selection['max_tests'] for selection in engine['selections']],
                         [190, 22, 160, 40])
        server = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-server'
                      and target.get('kind', 'lib') == 'lib'
                      and target.get('mode', 'test') == 'test')
        self.assertEqual(server['features'], ['core'])
        self.assertEqual(server['selection_groups'],
                         ['ordinary-conversion-name-budget', 'portable-path-consumers',
                          'typed-name-consumers'])
        self.assertEqual([selection['max_tests'] for selection in server['selections']],
                         [20, 160, 16])
        check = next(target for target in plan['rust_targets']
                     if target['package'] == 'storage-server' and target.get('mode') == 'check')
        self.assertFalse(check['default_features'])
        self.assertEqual(check['selection_groups'],
                         ['ordinary-conversion-name-budget', 'portable-path-consumers',
                          'typed-name-consumers'])
        http = next(target for target in plan['rust_targets'] if target.get('name') == 'http_engine')
        self.assertEqual(http['features'], ['core'])
        self.assertEqual([selection['prefixes'] for selection in http['selections']],
                         [['backup_namespace::'], ['history_alias_operations::']])

    def test_invalid_inventory_is_rejected(self):
        self.assertEqual(CI.inventory('module::test: test\n\n1 test, 0 benchmarks\n'), ['module::test'])
        for invalid in ['unexpected output', 'x: test\nx: test\n']:
            with self.assertRaises(ValueError):
                CI.inventory(invalid)


class TargetedBaseline(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        policy = self.root / CI.PLAN_PATH
        policy.parent.mkdir(parents=True)
        policy.write_text(json.dumps({
            'groups': {'fixture': {'reason': 'bounded fixture', 'python_tests': ['test_fixture'],
                                   'rust_targets': []}},
            'rules': [{'path': 'changed.py', 'groups': ['fixture']}],
            'unexecuted': 'All unrelated targets remain unexecuted.',
        }))
        self.event = self.root / 'event.json'
        self.base = 'a' * 40
        self.head = 'b' * 40
        self.calls = []
        self.ancestor = True
        self.run = CI.Run(self.root)
        self.addCleanup(mock.patch.stopall)
        mock.patch.dict(CI.os.environ, {'GITHUB_EVENT_NAME': 'workflow_dispatch',
                                      'GITHUB_EVENT_PATH': str(self.event),
                                      'GITHUB_STEP_SUMMARY': ''}).start()
        mock.patch.object(self.run, 'command', side_effect=self.command).start()

    def command(self, args, *unused):
        self.calls.append(args)
        if args[:2] == ['git', 'rev-parse']:
            return self.head if args[2] == 'HEAD' else self.base
        if args[:2] == ['git', 'merge-base']:
            if not self.ancestor:
                raise subprocess.CalledProcessError(1, args)
            return ''
        if args[:2] == ['git', 'diff']:
            return 'changed.py\0'
        self.fail(f'unexpected command: {args}')

    def dispatch(self, base='', scope='targeted'):
        self.event.write_text(json.dumps({'inputs': {'scope': scope, 'comparison_base': base}}))

    def test_explicit_ancestor_selects_only_mapped_work_and_records_base(self):
        self.dispatch(self.base)
        plan = self.run.plan()
        self.assertEqual(plan['groups'], ['fixture'])
        self.assertEqual(plan['rust_targets'], [])
        self.assertEqual(self.run.summary['base'], self.base)
        self.assertIn(['git', 'merge-base', '--is-ancestor', self.base, 'HEAD'], self.calls)
        self.assertNotIn(['git', 'rev-parse', 'HEAD^'], self.calls)

    def test_empty_dispatch_base_keeps_previous_commit_default(self):
        self.dispatch()
        self.run.plan()
        self.assertIn(['git', 'rev-parse', 'HEAD^'], self.calls)

    def test_push_uses_original_event_base_not_dispatch_input(self):
        self.dispatch('c' * 40)
        self.event.write_text(json.dumps({'before': self.base,
                                         'inputs': {'comparison_base': 'c' * 40}}))
        with mock.patch.dict(CI.os.environ, {'GITHUB_EVENT_NAME': 'push'}):
            self.run.plan()
        self.assertEqual(self.run.summary['base'], self.base)

    def test_invalid_sha_is_rejected_before_ancestor_or_diff(self):
        for value in ['main', 'abc', '-HEAD', 'A' * 40, 'a' * 40 + '\n', 3]:
            with self.subTest(value=value):
                self.calls.clear()
                self.dispatch(value)
                with self.assertRaises(ValueError):
                    self.run.plan()
                self.assertEqual(self.calls, [['git', 'rev-parse', 'HEAD']])

    def test_nonancestor_retains_failed_evidence_without_diff_or_build(self):
        self.dispatch(self.base)
        self.ancestor = False
        with self.assertRaises(subprocess.CalledProcessError):
            self.run.execute('plan')
        summary = json.loads((self.run.directory / 'summary.json').read_text())
        self.assertEqual(summary['status'], 'failed')
        self.assertIn('CalledProcessError', summary['error'])
        self.assertFalse(any(args[:2] == ['git', 'diff'] for args in self.calls))
        self.assertEqual(summary['passed'], [])

    def test_full_or_missing_scope_cannot_enter_targeted_runner(self):
        for scope in ['full', '', None]:
            with self.subTest(scope=scope):
                self.dispatch(self.base, scope)
                with self.assertRaises(ValueError):
                    self.run.plan()

    def test_matrix_emits_complete_exact_source_identity_without_executing_work(self):
        self.dispatch(self.base)
        output = self.root / 'outputs'
        with mock.patch.dict(CI.os.environ, {'GITHUB_OUTPUT': str(output)}):
            self.run.execute('matrix')
        value = json.loads(output.read_text().removeprefix('matrix='))
        request = value['include'][0]
        self.assertEqual(request['head'], self.head)
        self.assertEqual(request['base'], self.base)
        self.assertEqual(request['count'], 1)
        self.assertEqual(request['index'], 0)
        self.assertEqual(self.run.summary['status'], 'planned')
        self.assertEqual(self.run.summary['passed'], [])
        self.run.shard_request = request
        self.run.plan()
        self.assertEqual(self.run.summary['shard'], request)
        self.run.shard_request = request | {'base': 'c' * 40}
        with self.assertRaises(ValueError):
            self.run.plan()

class TargetedExecution(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / 'Cargo.lock').write_text('fixture lock\n')
        self.run = CI.Run(self.root)
        self.target = {'package': 'fixture', 'manifest': 'src/fixture/Cargo.toml',
                       'prefixes': ['module::'], 'max_tests': 2,
                       'selection_groups': ['fixture']}
        self.calls = []
        self.failures = {}
        self.names = ['module::a', 'module::b', 'other::unselected']
        self.ignored = []
        self.plan = {'python_tests': [], 'rust_targets': [self.target]}
        self.real_process = subprocess.run
        self.addCleanup(mock.patch.stopall)
        mock.patch.object(self.run, 'plan', return_value=self.plan).start()
        mock.patch.dict(CI.os.environ, {'GITHUB_STEP_SUMMARY': ''}).start()
        mock.patch.object(CI.subprocess, 'run', side_effect=self.process).start()

    def process(self, args, **kwargs):
        self.calls.append(args)
        key = args[args.index('--exact') + 1] if '--exact' in args else args[0]
        failure = self.failures.get(key)
        if isinstance(failure, Exception):
            raise failure
        if failure:
            kwargs['stderr'].write('fixture process failed\n')
            return subprocess.CompletedProcess(args, failure)
        if args[0] == 'cargo':
            package = args[args.index('-p') + 1]
            build = Path(args[args.index('--target-dir') + 1])
            build.mkdir(exist_ok=True)
            binary = build / package
            binary.write_text('current compiled fixture')
            artifact = {'reason': 'compiler-artifact', 'target': {'name': package,
                        'kind': ['lib']}, 'manifest_path': str(self.root / 'src' / package
                                                             / 'Cargo.toml'),
                        'profile': {'test': args[1] == 'test'}, 'features': [],
                        'executable': str(binary), 'filenames': [str(binary)]}
            output = json.dumps(artifact) + '\n' + json.dumps(
                {'reason': 'build-finished', 'success': True})
        elif '--list' in args:
            names = self.ignored if '--ignored' in args else self.names
            output = ''.join(name + ': test\n' for name in names)
        else:
            output = ('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; '
                      f'{len(self.names) - 1} filtered out; finished in 0.1s\n')
        kwargs['stdout'].write(output)
        return subprocess.CompletedProcess(args, 0)

    def persisted(self):
        return json.loads((self.run.directory / 'summary.json').read_text())

    def test_python_runner_uses_exact_budget_and_records_completion(self):
        directory = self.root / 'test/ci'
        directory.mkdir(parents=True)
        (directory / 'test_budget_fixture.py').write_text('# synthetic module\n')
        suite = mock.Mock()
        suite.countTestCases.return_value = 1
        observed = []

        def command(args, timeout):
            observed.append((args, timeout))
            (self.run.directory / '000.stderr.log').write_text(
                'Ran 1 test in 0.001s\n\nOK\n')

        with mock.patch.object(CI.unittest.defaultTestLoader, 'loadTestsFromName',
                               return_value=suite), \
                mock.patch.object(self.run, 'command', side_effect=command):
            self.run.python('test_budget_fixture', 600)
        self.assertEqual(observed[0][1], 600)
        self.assertIn('test_budget_fixture.py', observed[0][0])
        self.assertEqual(self.persisted()['passed'], [
            {'python_module': 'test_budget_fixture', 'tests': 1, 'timeout_seconds': 600},
        ])

    def test_failure_continues_selected_tests_and_target_but_still_raises(self):
        self.failures['module::a'] = 101
        check = {'package': 'consumer', 'manifest': 'src/consumer/Cargo.toml', 'mode': 'check'}
        self.plan['rust_targets'].append(check)
        with self.assertRaises(RuntimeError):
            self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['status'], 'failed')
        self.assertEqual([item['name'] for item in summary['passed']], ['module::b'])
        self.assertEqual(summary['compiled'], [check])
        self.assertEqual(summary['skipped'], [])
        self.assertEqual([(item['stage'], item['name']) for item in summary['failed']],
                         [('test', 'module::a')])
        executed = [args[args.index('--exact') + 1] for args in self.calls if '--exact' in args]
        self.assertEqual(executed, ['module::a', 'module::b'])
        self.assertEqual(summary['artifacts'][0]['selected'], executed)
        self.assertEqual(summary['artifacts'][0]['unexecuted'], ['other::unselected'])

    def test_failed_compile_never_runs_stale_binary_and_continues_next_target(self):
        stale = self.run.directory / 'build/fixture'
        stale.parent.mkdir()
        stale.write_text('stale binary')
        second = dict(self.target, package='consumer', manifest='src/consumer/Cargo.toml')
        self.plan['rust_targets'].append(second)
        original = self.process
        def process(args, **kwargs):
            if args[:2] == ['cargo', 'test'] and args[args.index('-p') + 1] == 'fixture':
                self.calls.append(args)
                return subprocess.CompletedProcess(args, 101)
            return original(args, **kwargs)
        with mock.patch.object(CI.subprocess, 'run', side_effect=process):
            with self.assertRaises(RuntimeError):
                self.run.execute('run')
        summary = self.persisted()
        self.assertFalse(any(args[0] == str(stale) for args in self.calls))
        self.assertEqual([item['configuration'] for item in summary['passed']], [second, second])
        self.assertEqual(summary['failed'][0]['stage'], 'compile')
        self.assertEqual(summary['skipped'][0]['configuration'], self.target)

    def test_inventory_and_selection_failures_are_failed_not_successful_skips(self):
        cases = [(['module::a', 'module::a'], [], 2, 'inventory'),
                 (['other::unselected'], [], 2, 'selection'),
                 (['module::a'], ['module::a'], 2, 'selection'),
                 (['module::a', 'module::b'], [], 1, 'selection')]
        for names, ignored, maximum, stage in cases:
            with self.subTest(stage=stage, names=names):
                self.run = CI.Run(self.root)
                self.names, self.ignored = names, ignored
                self.target['max_tests'] = maximum
                with mock.patch.object(self.run, 'plan', return_value=self.plan):
                    with self.assertRaises(RuntimeError):
                        self.run.execute('run')
                summary = self.persisted()
                self.assertEqual(summary['status'], 'failed')
                self.assertEqual(summary['failed'][0]['stage'], stage)
                self.assertEqual(summary['passed'], [])
                self.assertEqual(summary['compiled'], [])
                self.assertEqual(len(summary['skipped']), 1)
        self.assertFalse(any('--exact' in args for args in self.calls))

    def test_launch_error_is_classified_and_does_not_stop_next_test(self):
        self.failures['module::a'] = OSError('fixture launch error')
        with self.assertRaises(RuntimeError):
            self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['failed'][0]['kind'], 'OSError')
        self.assertEqual(summary['commands'][3]['status'], 'error')
        self.assertEqual([item['name'] for item in summary['passed']], ['module::b'])
        self.assertEqual(summary['blocked'], [])

    def test_test_timeout_preserves_completed_results_and_blocks_remaining_scope(self):
        self.names.insert(2, 'module::c')
        self.target['max_tests'] = 3
        self.failures['module::b'] = subprocess.TimeoutExpired(['fixture'], 1)
        later = dict(self.target, package='consumer', manifest='src/consumer/Cargo.toml')
        self.plan['rust_targets'].append(later)
        with self.assertRaises(subprocess.TimeoutExpired):
            self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['status'], 'failed')
        self.assertEqual(summary['failed'][0]['kind'], 'timeout')
        self.assertEqual(summary['commands'][-1]['status'], 'timed-out')
        self.assertEqual([item['name'] for item in summary['passed']], ['module::a'])
        self.assertEqual(summary['blocked'][0]['names'], ['module::c'])
        self.assertEqual(summary['blocked'][1]['rust_targets'], [later])
        executed = [args[args.index('--exact') + 1] for args in self.calls if '--exact' in args]
        self.assertEqual(executed, ['module::a', 'module::b'])
        self.assertEqual(len([args for args in self.calls if args[0] == 'cargo']), 1)

    def test_real_subprocess_timeout_stops_before_next_selected_test(self):
        original = self.process
        def process(args, **kwargs):
            if '--exact' in args:
                self.calls.append(args)
                kwargs['timeout'] = 0.1
                return self.real_process([CI.sys.executable, '-c',
                                          'import time; time.sleep(30)'], **kwargs)
            return original(args, **kwargs)
        later = dict(self.target, package='consumer', manifest='src/consumer/Cargo.toml')
        self.plan['rust_targets'].append(later)
        with mock.patch.object(CI.subprocess, 'run', side_effect=process):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['status'], 'failed')
        self.assertEqual(summary['commands'][-1]['status'], 'timed-out')
        self.assertEqual(summary['failed'][0]['kind'], 'timeout')
        self.assertEqual(summary['failed'][0]['name'], 'module::a')
        self.assertEqual(summary['passed'], [])
        self.assertEqual(summary['blocked'][0]['names'], ['module::b'])
        self.assertEqual(summary['blocked'][1]['rust_targets'], [later])
        self.assertEqual(len(self.calls), 4)

    def test_compile_and_inventory_timeout_stop_all_later_targets(self):
        later = dict(self.target, package='consumer', manifest='src/consumer/Cargo.toml')
        self.plan['rust_targets'].append(later)
        for stage, key in [('compile', 'cargo'),
                           ('inventory', str(self.run.directory / 'build/fixture'))]:
            with self.subTest(stage=stage):
                self.run = CI.Run(self.root)
                self.calls.clear()
                self.failures = {key: subprocess.TimeoutExpired([key], 1)}
                with mock.patch.object(self.run, 'plan', return_value=self.plan):
                    with self.assertRaises(subprocess.TimeoutExpired):
                        self.run.execute('run')
                summary = self.persisted()
                self.assertEqual(summary['failed'][0]['stage'], stage)
                self.assertEqual(summary['commands'][-1]['status'], 'timed-out')
                self.assertEqual(summary['passed'], [])
                self.assertEqual(summary['blocked'][0]['configuration'], self.target)
                self.assertEqual(summary['blocked'][1]['rust_targets'], [later])
                self.assertEqual(len(self.calls), 1 if stage == 'compile' else 2)

    def test_python_timeout_stops_remaining_modules_and_all_rust_targets(self):
        self.plan['python_tests'] = ['test_first', 'test_timeout', 'test_later']
        self.plan['python_timeouts'] = {'test_timeout': 600}
        visited = []
        def python(name, timeout=120):
            visited.append(name)
            if name == 'test_timeout':
                self.assertEqual(timeout, 600)
                self.run.command(['python-fixture'], timeout)
            else:
                self.assertEqual(timeout, 120)
            self.run.summary['passed'].append({'python_module': name, 'tests': 1})
        self.failures['python-fixture'] = subprocess.TimeoutExpired(['python-fixture'], 1)
        with mock.patch.object(self.run, 'python', side_effect=python):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(visited, ['test_first', 'test_timeout'])
        self.assertEqual(summary['failed'][0]['stage'], 'python')
        self.assertEqual(summary['commands'][-1]['status'], 'timed-out')
        self.assertEqual(summary['passed'], [{'python_module': 'test_first', 'tests': 1}])
        self.assertEqual(summary['blocked'][0]['python_modules'], ['test_later'])
        self.assertEqual(summary['blocked'][0]['rust_targets'], [self.target])
        self.assertEqual(self.calls, [['python-fixture']])

    def test_toolchain_timeout_blocks_native_and_cross_targets(self):
        cross = dict(self.target, mode='check', compile_target='wasm32-unknown-unknown')
        self.plan['rust_targets'].insert(0, cross)
        self.failures['rustup'] = subprocess.TimeoutExpired(['rustup'], 1)
        with self.assertRaises(subprocess.TimeoutExpired):
            self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['failed'][0]['stage'], 'toolchain')
        self.assertEqual(summary['commands'][-1]['status'], 'timed-out')
        self.assertEqual(summary['blocked'][0]['rust_targets'], [cross, self.target])
        self.assertEqual(summary['passed'], [])
        self.assertEqual(len(self.calls), 1)

    def test_python_failure_continues_other_module_and_selected_rust(self):
        self.plan['python_tests'] = ['test_first', 'test_second']
        visited = []
        def python(name, timeout=120):
            visited.append(name)
            if name == 'test_first':
                raise ValueError('fixture invalid Python evidence')
            self.run.summary['passed'].append({'python_module': name, 'tests': 1})
        with mock.patch.object(self.run, 'python', side_effect=python):
            with self.assertRaises(RuntimeError):
                self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(visited, self.plan['python_tests'])
        self.assertEqual(summary['failed'][0]['stage'], 'python')
        self.assertEqual(len(summary['passed']), 3)

    def test_toolchain_failure_skips_only_cross_target(self):
        cross = dict(self.target, mode='check', compile_target='wasm32-unknown-unknown')
        self.plan['rust_targets'].insert(0, cross)
        self.failures['rustup'] = 1
        with self.assertRaises(RuntimeError):
            self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['failed'][0]['stage'], 'toolchain')
        self.assertEqual(summary['skipped'][0]['configuration'], cross)
        self.assertEqual(len(summary['passed']), 2)
        self.assertFalse(any('--target' in args for args in self.calls))

    def test_success_and_compile_only_evidence_stay_distinct(self):
        self.plan['rust_targets'].append(dict(self.target, mode='check'))
        self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['status'], 'selected-tests-passed')
        self.assertEqual(len(summary['passed']), 2)
        self.assertEqual(len(summary['compiled']), 1)
        self.assertEqual(summary['failed'], [])
        self.assertEqual(summary['skipped'], [])

    def test_failed_run_exits_nonzero_after_retaining_later_pass(self):
        script = """
import json
import sys
sys.path.insert(0, 'test/ci')
from test_targeted_ci import TargetedExecution
case = TargetedExecution()
case.setUp()
case.failures['module::a'] = 101
try:
    case.run.execute('run')
finally:
    print('RESULT_JSON=' + json.dumps(case.persisted()))
    case.doCleanups()
"""
        # Popen is not the mocked subprocess.run used by the synthetic Cargo boundary.
        with subprocess.Popen([CI.sys.executable, '-c', script], cwd=ROOT,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              text=True) as child:
            stdout, stderr = child.communicate(timeout=30)
        self.assertNotEqual(child.returncode, 0, stderr)
        results = [line.removeprefix('RESULT_JSON=') for line in stdout.splitlines()
                   if line.startswith('RESULT_JSON=')]
        self.assertEqual(len(results), 1, stderr)
        summary = json.loads(results[0])
        self.assertEqual(summary['status'], 'failed')
        self.assertEqual([item['name'] for item in summary['passed']], ['module::b'])
        self.assertEqual(summary['failed'][0]['name'], 'module::a')

    def test_zero_exit_with_invalid_test_evidence_still_fails_and_continues(self):
        original = self.process
        def process(args, **kwargs):
            if '--exact' in args and args[args.index('--exact') + 1] == 'module::a':
                self.calls.append(args)
                kwargs['stdout'].write('test result: ok. 0 passed; 0 failed; 0 ignored; '
                                       '0 measured; 3 filtered out; finished in 0.1s\n')
                return subprocess.CompletedProcess(args, 0)
            return original(args, **kwargs)
        with mock.patch.object(CI.subprocess, 'run', side_effect=process):
            with self.assertRaises(RuntimeError):
                self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['failed'][0]['stage'], 'test')
        self.assertEqual([item['name'] for item in summary['passed']], ['module::b'])

    def test_modified_binary_stops_remaining_tests_of_only_that_target(self):
        original = self.process
        def process(args, **kwargs):
            result = original(args, **kwargs)
            if '--exact' in args:
                Path(args[0]).write_text('replaced executable')
            return result
        check = {'package': 'consumer', 'manifest': 'src/consumer/Cargo.toml', 'mode': 'check'}
        self.plan['rust_targets'].append(check)
        with mock.patch.object(CI.subprocess, 'run', side_effect=process):
            with self.assertRaises(RuntimeError):
                self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['failed'][0]['stage'], 'integrity')
        self.assertEqual(summary['skipped'][0]['names'], ['module::b'])
        self.assertEqual([item['name'] for item in summary['passed']], ['module::a'])
        self.assertEqual(summary['compiled'], [check])

    def test_large_evidence_is_complete_in_artifact_but_compact_in_step_summary(self):
        large = {'inventory': ['unselected::' + '界' * 1000] * 200}
        self.run.summary['artifacts'].append(large)
        path = self.root / 'step-summary.md'
        with mock.patch.dict(CI.os.environ, {'GITHUB_STEP_SUMMARY': str(path)}):
            self.run.execute('run')
        evidence = self.persisted()
        self.assertEqual(evidence['artifacts'][0], large)
        self.assertGreater(len(json.dumps(evidence).encode('utf-8')), 1024 * 1024)
        self.assertEqual(evidence['status'], 'selected-tests-passed')
        self.assertEqual(len(evidence['passed']), 2)
        rendered = path.read_text()
        self.assertLess(len(rendered.encode('utf-8')), 128 * 1024)
        self.assertIn('"rust_tests_passed": 2', rendered)
        self.assertIn('summary.json', rendered)
        self.assertNotIn('unselected::', rendered)

    def test_failure_presentation_is_bounded_and_does_not_mutate_evidence(self):
        failure = {key: '界' * 100000 for key in
                   ('stage', 'name', 'python_module', 'error')}
        summary = {'status': 'failed', 'failed': [failure] * 100,
                   'error': '界' * 100000, 'passed': [], 'skipped': [{}] * 30,
                   'blocked': [{}] * 40}
        before = copy.deepcopy(summary)
        rendered = CI.step_summary(summary)
        self.assertLess(len(rendered.encode('utf-8')), 256 * 1024)
        self.assertIn('"failed": 100', rendered)
        self.assertIn('"skipped": 30', rendered)
        self.assertIn('"blocked": 40', rendered)
        self.assertEqual(summary, before)

    def test_invalid_plan_stops_before_any_execution(self):
        with mock.patch.object(self.run, 'plan', side_effect=ValueError('invalid selection')):
            with self.assertRaises(ValueError):
                self.run.execute('run')
        self.assertEqual(self.persisted()['status'], 'failed')
        self.assertEqual(self.calls, [])


DIRECTORY_RENAME_PATHS = [
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
        for path in DIRECTORY_RENAME_PATHS + ['test/ci/test_directory_rename_targeted_ci.py']:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('directory-rename-recovery', plan['groups'])
                self.assertIn('test_targeted_ci', plan['python_tests'])
                self.assertLessEqual(len(plan['rust_targets']), 16)

    def test_complete_candidate_retains_existing_independent_selection_budgets(self):
        plan = CI.select_plan(self.policy, DIRECTORY_RENAME_PATHS + [
            '.github/ci/targeted-tests.json',
            'test/ci/test_directory_rename_targeted_ci.py',
            'docs/validation/directory-rename-recovery.md',
        ])
        self.assertIn('historical-directory-web', plan['groups'])
        self.assertIn('web-recovery-principals', plan['groups'])
        self.assertEqual(len(plan['rust_targets']), 6)
        browser_targets = [target for target in plan['rust_targets']
                           if target['package'] == 'browser-smoke']
        self.assertCountEqual([
            (target['manifest'], target['kind'], target['name'], target.get('mode', 'test'))
            for target in browser_targets
        ], [
            ('tools/browser-smoke/Cargo.toml', 'bin', 'browser-smoke', 'test'),
            ('tools/browser-smoke/Cargo.toml', 'bin', 'browser-smoke', 'check'),
        ])
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
        self.assertNotIn('files::mkdir::storage::rename_tests::', native['prefixes'])
        self.assertIn('files::mkdir::storage::tests::', native['prefixes'])
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
            'selection_ingress_binds_path_and_parses_operation_once',
            'engine_backend::read_projection::tests::default_directory_tests::'
            'typed_directory_rename_keeps_canonical_receipts_and_cached_name_collision_policy',
        ])

    def test_http_retirement_has_closed_routes_and_retained_behavior_within_budget(self):
        paths = [
            'src/server/src/lib.rs',
            'src/server/src/http_default_directory.rs',
            'test/server/unit/http_default_directory.rs',
            'test/server/unit/backend_admission.rs',
            'test/server/unit/tests/http_boundary.rs',
            'test/server/integration/http_engine/optional_identities.rs',
            'test/server/integration/http_engine.rs',
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('directory-rename-http-retirement', plan['groups'])
        plan = CI.select_plan(self.policy, paths + [
            '.github/ci/targeted-tests.json',
            'test/ci/test_directory_rename_targeted_ci.py',
            'docs/15-api-and-errors.md',
            'docs/contracts/openapi.json',
            'docs/design/directory-rename-http-retirement.md',
        ])
        self.assertEqual(len(plan['rust_targets']), 14)
        self.assertEqual(len(plan['python_tests']), 8)
        group = self.policy['groups']['directory-rename-http-retirement']
        native, http, product, no_core = group['rust_targets']
        self.assertEqual(native['max_tests'], 5)
        self.assertIn(
            'engine_backend::read_projection::tests::default_directory_tests::'
            'typed_directory_rename_keeps_canonical_receipts_and_cached_name_collision_policy',
            native['exact'])
        self.assertEqual(http['kind'], 'test')
        self.assertEqual(http['name'], 'http_engine')
        self.assertEqual(http['exact'], [
            'optional_identities::'
            'retired_directory_rename_route_cannot_publish_or_change_current_reads',
            'public_file_branch_label_patch_preserves_content_and_directory_nodes',
            'default_directory_branch_is_shared_authorized_and_persistent',
        ])
        self.assertEqual(http['max_tests'], 3)
        self.assertEqual(product['mode'], 'check')
        self.assertEqual(product['features'], ['core'])
        self.assertEqual(no_core['mode'], 'check')
        self.assertFalse(no_core['default_features'])


class TargetedShards(unittest.TestCase):
    """Execution receipts, not job names, prove complete selected coverage."""

    def setUp(self):
        self.shards = CI.shard_tools()
        targets = [
            {'package': 'fixture', 'manifest': 'src/fixture/Cargo.toml',
             'kind': 'test', 'name': f'fixture_{index}', 'mode': 'check'}
            for index in range(17)
        ]
        targets[0] = {
            'package': 'fixture', 'manifest': 'src/fixture/Cargo.toml',
            'prefixes': ['selected::'], 'max_tests': 2, 'timeout_seconds': 900,
        }
        self.policy = {
            'groups': {'fixture': {
                'reason': 'Synthetic cross-crate atomic change',
                'python_tests': ['test_fixture_' + suffix for suffix in 'abcdefghi'],
                'rust_targets': targets,
            }},
            'rules': [{'path': 'changed.rs', 'groups': ['fixture']}],
            'unexecuted': 'Unrelated tests are not selected.',
        }
        self.plan = CI.select_plan(self.policy, ['changed.rs'], shardable=True)
        self.summary = {'head': 'b' * 40, 'base': 'a' * 40, 'policy_sha256': 'c' * 64}

    def receipts(self):
        receipts = []
        for request in self.shards.matrix(self.summary, self.plan)['include']:
            receipt = dict(self.summary, status='selected-tests-passed',
                           failed=[], skipped=[], blocked=[], compiled=[], artifacts=[], passed=[])
            plan = self.shards.select(receipt, self.plan, request)
            receipt['plan'] = plan
            for module in plan['python_tests']:
                receipt['passed'].append({'python_module': module, 'tests': 1,
                                         'timeout_seconds': plan['python_timeouts'][module]})
            for target in plan['rust_targets']:
                artifact = {'configuration': target}
                if target.get('mode') == 'check':
                    artifact['files'] = [{'path': '/repo/target/targeted-ci/build/fixture', 'sha256': 'd' * 64}]
                    receipt['compiled'].append(target)
                else:
                    artifact.update(inventory=['selected::a', 'selected::b', 'other::c'],
                                    ignored=[], selected=['selected::a', 'selected::b'],
                                    path='/repo/target/targeted-ci/build/fixture', sha256='d' * 64,
                                    package=target['package'], unexecuted=['other::c'])
                    artifact['selections'] = CI.selection_evidence(
                        artifact['inventory'], artifact['ignored'], target)
                    artifact['test_timeouts'] = CI.test_timeouts(artifact['selections'])
                    receipt['passed'].extend({'configuration': target, 'name': name}
                                              for name in artifact['selected'])
                receipt['artifacts'].append(artifact)
            receipts.append(receipt)
        return receipts

    def verify(self, receipts):
        return self.shards.verify(
            self.summary, self.plan, receipts, CI.selection_evidence, CI.test_timeouts)

    def test_default_remains_bounded_and_explicit_partition_preserves_exact_union(self):
        with self.assertRaises(ValueError):
            CI.select_plan(self.policy, ['changed.rs'])
        shards = self.shards.partition(self.plan)
        self.assertEqual(len(shards), 2)
        self.assertEqual([target for shard in shards for target in shard['rust_targets']],
                         self.plan['rust_targets'])
        self.assertEqual([module for shard in shards for module in shard['python_tests']],
                         self.plan['python_tests'])
        for shard in shards:
            self.assertLessEqual(len(shard['rust_targets']), 16)
            self.assertLessEqual(len(shard['python_tests']), 8)
        self.assertEqual(self.shards.partition(copy.deepcopy(self.plan)), shards)
        self.assertEqual(self.verify(self.receipts())['status'], 'all-selected-shards-passed')

    def test_sharding_never_relaxes_target_prefix_timeout_or_total_resource_bounds(self):
        for patch in ({'prefixes': [f'prefix_{i}::' for i in range(17)]},
                      {'timeout_seconds': 901}, {'max_tests': 193}):
            policy = copy.deepcopy(self.policy)
            policy['groups']['fixture']['rust_targets'][0].update(patch)
            with self.assertRaises(ValueError):
                CI.select_plan(policy, ['changed.rs'], shardable=True)
        oversized = copy.deepcopy(self.plan)
        oversized['rust_targets'] *= 16
        with self.assertRaises(ValueError):
            self.shards.partition(oversized)
        policy = copy.deepcopy(self.policy)
        policy['rules'] = [{'path': 'different.rs', 'groups': ['fixture']}]
        with self.assertRaises(ValueError):
            CI.select_plan(policy, ['changed.rs'], shardable=True)

    def test_shard_identity_rejects_other_commit_base_policy_plan_and_index(self):
        original = self.shards.matrix(self.summary, self.plan)['include'][0]
        for patch in ({'head': 'd' * 40}, {'base': 'e' * 40}, {'policy_sha256': 'f' * 64},
                      {'plan_sha256': '0' * 64}, {'index': -1}, {'index': 2},
                      {'index': True}, {'count': True}, {'count': 3}, {'extra': 1}):
            with self.subTest(patch=patch), self.assertRaises(ValueError):
                self.shards.select(dict(self.summary), self.plan, original | patch)
        with self.assertRaises(ValueError):
            self.shards.select(dict(self.summary), self.plan, {})

    def test_missing_duplicate_failed_or_wrong_source_receipts_cannot_pass(self):
        receipts = self.receipts()
        for bad in (receipts[:-1], receipts + [receipts[0]], [receipts[0], receipts[0]]):
            with self.assertRaises(ValueError):
                self.verify(bad)
        for field, value in [('status', 'planned'), ('head', 'e' * 40),
                             ('base', 'e' * 40), ('policy_sha256', 'e' * 64),
                             ('failed', [{'error': 'test failed'}]),
                             ('skipped', [{'reason': 'missing executable'}]),
                             ('blocked', [{'reason': 'timeout'}])]:
            bad = copy.deepcopy(receipts)
            bad[0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.verify(bad)

    def test_green_label_without_exact_python_rust_or_compile_evidence_is_rejected(self):
        original = self.receipts()
        mutations = [
            lambda receipt: receipt['passed'].pop(),
            lambda receipt: receipt['passed'].append(receipt['passed'][0]),
            lambda receipt: receipt['passed'][0].update(tests=0),
            lambda receipt: receipt['passed'][0].update(timeout_seconds=900),
            lambda receipt: receipt['compiled'].pop(),
            lambda receipt: receipt['artifacts'].pop(),
            lambda receipt: receipt['artifacts'][0]['selected'].pop(),
            lambda receipt: receipt['artifacts'][0]['inventory'].remove('selected::b'),
            lambda receipt: receipt['plan']['rust_targets'].pop(),
        ]
        for mutate in mutations:
            bad = copy.deepcopy(original)
            mutate(bad[0])
            with self.assertRaises(ValueError):
                self.verify(bad)

    def test_current_artifact_identity_and_selection_budget_evidence_is_required(self):
        original = self.receipts()
        for field in ('path', 'sha256', 'package', 'selections', 'test_timeouts', 'unexecuted'):
            bad = copy.deepcopy(original)
            del bad[0]['artifacts'][0][field]
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.verify(bad)
        for patch in ({'sha256': 'bad'}, {'path': '/outside/fixture'},
                      {'test_timeouts': {'selected::a': 901, 'selected::b': 900}},
                      {'package': 'other'}, {'unexecuted': []}):
            bad = copy.deepcopy(original)
            bad[0]['artifacts'][0].update(patch)
            with self.subTest(patch=patch), self.assertRaises(ValueError):
                self.verify(bad)
        for files in ([{}], [], [{'path': '/repo/target/targeted-ci/build/fixture'}]):
            bad = copy.deepcopy(original)
            bad[0]['artifacts'][1]['files'] = files
            with self.subTest(files=files), self.assertRaises(ValueError):
                self.verify(bad)

    def test_existing_multigroup_selections_are_not_flattened_or_dropped(self):
        self.policy['groups']['also'] = {
            'reason': 'Second owner retains its own timeout and budget', 'python_tests': [],
            'rust_targets': [{'package': 'fixture', 'manifest': 'src/fixture/Cargo.toml',
                              'exact': ['selected::b'], 'max_tests': 1,
                              'timeout_seconds': 200}],
        }
        self.policy['rules'][0]['groups'].append('also')
        plan = CI.select_plan(self.policy, ['changed.rs'], shardable=True)
        shards = self.shards.partition(plan)
        target = shards[0]['rust_targets'][0]
        self.assertEqual(target['selection_groups'], ['also', 'fixture'])
        evidence = CI.selection_evidence(['selected::a', 'selected::b'], [], target)
        self.assertEqual(CI.test_timeouts(evidence), {'selected::a': 900, 'selected::b': 200})
        self.assertEqual(target, plan['rust_targets'][0])


class ContainerTagsSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_wire_capture_destination_is_owned_by_the_exact_run_evidence_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            run = CI.Run(Path(directory))
            wire = Path(run.environment['ZXS_TEST_WIRE_EVIDENCE_DIR'])
            self.assertEqual(wire, run.directory / 'wire')
            self.assertTrue(wire.is_dir())
            self.assertEqual(run.summary['wire_evidence_directory'], str(wire))
            self.assertEqual(Path(run.environment['TMPDIR']), run.directory / 'fixtures')

    def test_each_authority_selects_its_container_regressions_without_new_file_cochange(self):
        authorities = {
            'container-tags-contract': [
                'src/contract/src/error.rs',
                'src/contract/src/model.rs', 'src/contract/src/tracked.rs',
                'src/contract/src/file_container_tags.rs',
                'docs/tools/contract-check/tests/fixtures/merge-tags-backup.md',
            ],
            'container-tags-storage': [
                'src/engine/src/journal/snapshot.rs', 'src/engine/src/state_projection.rs',
                'src/engine/src/history_package/manifest.rs',
                'src/engine/src/history_package/state_read.rs',
                'src/engine/src/container_storage/changes.rs',
                'src/engine/src/container_storage/file_database/rows.rs',
                'src/engine/src/engine/index/system_changes/catalog.rs',
                'src/engine/src/file_deletion/freeze.rs',
                'src/engine/src/foreign_import/identity_closure/graph/manifest.rs',
                'src/engine/src/maintenance.rs',
                'test/engine/integration/foreign_original_databases.rs',
            ],
            'container-tags-engine': [
                'src/engine/src/engine/history_window/projection.rs',
                'src/engine/src/business/file_tags.rs',
                'src/engine/src/persist/container_tags.rs',
                'src/engine/src/publication_shape.rs',
                'src/engine/src/engine/planning.rs',
                'src/engine/src/engine/recovery.rs',
                'src/engine/src/engine/disk_registry/current/file_tags/publication.rs',
            ],
            'container-tags-http': [
                'src/server/src/engine_backend.rs',
                'src/server/src/backend_methods.rs',
                'src/server/src/file_manager.rs',
                'src/server/src/http_boundary.rs',
                'src/server/src/http_file_tags.rs', 'src/server/src/operation_wire.rs',
                'src/server/src/backend_file_tags.rs', 'src/client/src/lib.rs',
            ],
            'container-tags-web': [
                'src/web/src/files/model.rs',
                'src/web/src/files/detail.rs',
                'src/web/src/ui.rs',
                'src/web/src/files/history/ingress.rs',
                'src/web/src/files/tags/mod.rs', 'src/web/src/files/tags/view.rs',
                'src/web/src/files/review.rs',
            ],
            'container-tags-browser': [
                'test/tools/browser-smoke/runner/phase1_files.rs',
                'test/tools/browser-smoke/runner/phase1_tags.js',
                'test/tools/browser-smoke/runner/tag_unknown.rs',
                'test/tools/browser-smoke/runner/tag_unknown.js',
                'test/tools/browser-smoke/runner/history_windows.rs',
                'test/tools/browser-smoke/unit/phase1_files_probe_test.mjs',
                'test/tools/browser-smoke/unit/tag_unknown_probe_test.mjs',
                'test/tools/browser-smoke/unit/phase1_files_tests.rs',
                'test/tools/browser-smoke/unit/tag_unknown_tests.rs',
                'docs/tools/browser-smoke.md',
                'test/ci/test_phase1_file_probes.py',
                'test/ci/test_tag_unknown_probe.py',
                'test/ci/test_browser_history_window_wait.py',
            ],
            'container-tags-format': [
                'src/engine/src/disk.rs',
                'src/engine/src/engine/membership_transition/pending.rs',
                'src/engine/src/engine/membership_transition/archive/persistent.rs',
                'src/engine/src/engine/backup_preparation/native_control/archive.rs',
            ],
        }
        for group, paths in authorities.items():
            for path in paths:
                with self.subTest(path=path):
                    plan = CI.select_plan(self.policy, [path], shardable=True)
                    self.assertIn(group, plan['groups'])
                    selected = [selection
                                for target in plan['rust_targets']
                                for origin, selection in zip(
                                    target['selection_groups'], target.get('selections', [
                                        {key: value for key, value in target.items()
                                         if key != 'selection_groups'}]))
                                if origin == group]
                    self.assertCountEqual(selected, self.policy['groups'][group]['rust_targets'])

    def test_foreign_nonempty_tag_copy_fixture_selects_its_real_integration_target(self):
        path = 'test/engine/integration/foreign_original_databases.rs'
        plan = CI.select_plan(self.policy, [path])
        self.assertEqual(plan['groups'], ['container-tags-storage'])
        target = next(target for target in plan['rust_targets']
                      if target.get('name') == 'foreign_original_databases')
        self.assertEqual(target['package'], 'storage-engine')
        self.assertEqual(target['kind'], 'test')
        self.assertTrue(target['whole'])
        self.assertEqual(target['max_tests'], 24)
        self.assertEqual(target['timeout_seconds'], 900)

    def test_atomic_change_keeps_every_old_and_new_selection_in_same_commit_shards(self):
        paths = [
            'test/engine/unit/business/file_tags/tests.rs', 'src/contract/src/lib.rs',
            'src/engine/src/disk.rs', 'src/contract/src/model.rs',
            'src/engine/src/journal/snapshot.rs', 'src/engine/src/business/file_tags.rs',
            'src/server/src/http_file_tags.rs', 'src/web/src/files/tags/mod.rs',
            'test/tools/browser-smoke/runner/history_windows.rs',
            CI.PLAN_PATH, 'test/ci/test_targeted_ci.py',
        ]
        with self.assertRaises(ValueError):
            CI.select_plan(self.policy, paths)
        plan = CI.select_plan(self.policy, paths, shardable=True)
        self.assertTrue({'portable-path-consumers', 'paged-deletion-review',
                         'held-directory-layout', 'ci-selection'} <= set(plan['groups']))
        self.assertTrue({'container-tags-' + area for area in
                         ('contract', 'storage', 'engine', 'http', 'web', 'format', 'browser')}
                        <= set(plan['groups']))
        self.assertGreater(len(plan['rust_targets']), 16)
        shards = CI.shard_tools().partition(plan)
        self.assertEqual([target for shard in shards for target in shard['rust_targets']],
                         plan['rust_targets'])
        self.assertEqual([name for shard in shards for name in shard['python_tests']],
                         plan['python_tests'])
        for shard in shards:
            self.assertLessEqual(len(shard['rust_targets']), 16)
            self.assertLessEqual(len(shard['python_tests']), 8)
        for group in plan['groups']:
            actual = [selection for target in plan['rust_targets']
                      for origin, selection in zip(target['selection_groups'],
                          target.get('selections', [{key: value for key, value in target.items()
                                                    if key != 'selection_groups'}]))
                      if origin == group]
            self.assertCountEqual(actual, self.policy['groups'][group]['rust_targets'])

    def test_tag_sources_keep_product_features_cross_target_and_current_format_boundaries(self):
        groups = self.policy['groups']
        web = groups['container-tags-web']['rust_targets']
        self.assertTrue(any(target.get('compile_target') == 'wasm32-unknown-unknown'
                            for target in web))
        engine = groups['container-tags-engine']['rust_targets']
        self.assertTrue(any(target.get('mode') == 'check'
                            and target.get('default_features') is False for target in engine))
        http = groups['container-tags-http']['rust_targets']
        self.assertTrue(any(target['package'] == 'storage-client'
                            and target.get('prefixes') == ['container_tags_tests::']
                            for target in http))
        self.assertTrue(all(target['features'] == ['core'] for target in http
                            if target['package'] == 'storage-server'
                            and target.get('mode', 'test') == 'test'))
        self.assertEqual(http[0]['exact'], [
            'authorized_backend_tests::protected_overrides_preserve_identity_arguments_results_and_errors',
            'authorized_backend_tests::revoked_sessions_never_reach_protected_backend_methods',
        ])
        browser = groups['container-tags-browser']
        self.assertEqual(set(browser['python_tests']), {
            'test_phase1_file_probes', 'test_tag_unknown_probe',
            'test_browser_history_window_wait',
        })
        self.assertEqual(browser['rust_targets'][0]['prefixes'],
                         ['phase1_files::tests::', 'tag_unknown::tests::'])
        formats = groups['container-tags-format']['rust_targets'][0]['prefixes']
        self.assertIn('engine::membership_transition::pending::terminal_resume::', formats)
        self.assertIn('engine::backup_preparation::native_control::archive::format_tests::', formats)
        for name, group in groups.items():
            if name.startswith('container-tags-'):
                for target in group['rust_targets']:
                    self.assertLessEqual(len(target.get('prefixes', [])), 16)
                    self.assertLessEqual(len(target.get('exact', [])), 16)
                    self.assertLessEqual(target.get('timeout_seconds', 300), 900)


if __name__ == '__main__':
    unittest.main()
