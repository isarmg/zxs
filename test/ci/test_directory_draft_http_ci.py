"""Keep the actual HTTP/Born route bounded, additive and backed by real core."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'directory_draft_http_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)
GROUP = 'directory-draft-http'
LEAVES = [
    'src/contract/src/snapshot_worktree/draft_commit.rs',
    'src/engine/src/snapshot_worktree/public/drafts.rs',
    'src/server/src/engine_backend/snapshot_worktree/drafts.rs',
    'src/server/src/http_snapshot_worktree/drafts.rs',
    'test/contract/unit/directory_draft_commit.rs',
    'test/server/integration/phase1_save_tags/directory_drafts.rs',
    'test/ci/test_directory_draft_http_ci.py',
]


class DirectoryDraftHttpSelection(unittest.TestCase):
    def test_each_real_leaf_selects_scoped_http_and_strict_contract_without_full_suites(self):
        policy = json.loads((ROOT / CI.PLAN_PATH).read_text())
        expected = policy['groups'][GROUP]['rust_targets']
        self.assertEqual(len(expected), 4)
        contract, http, core, coreless = expected
        self.assertEqual(contract['exact'], [
            'snapshot_worktree::draft_commit::tests::'
            'born_selection_wire_is_strict_and_public_outcome_has_no_native_material'])
        self.assertEqual(http['exact'], [
            'directory_drafts::'
            'authenticated_directory_draft_commit_is_scoped_selected_and_cold_idempotent'])
        self.assertEqual(http['name'], 'phase1_save_tags')
        self.assertEqual(http['features'], ['core'])
        self.assertEqual([contract['max_tests'], http['max_tests']], [1, 1])
        self.assertEqual([core['mode'], coreless['mode']], ['check', 'check'])
        self.assertTrue(core['default_features'])
        self.assertFalse(coreless['default_features'])
        for path in LEAVES:
            with self.subTest(path=path):
                plan = CI.select_plan(policy, [path], shardable=True)
                self.assertIn(GROUP, plan['groups'])
                self.assertIn('test_directory_draft_http_ci', plan['python_tests'])
                actual = [selection for target in plan['rust_targets']
                          for origin, selection in zip(target['selection_groups'],
                              target.get('selections', [{key: value for key, value in target.items()
                                                        if key != 'selection_groups'}]))
                          if origin == GROUP]
                self.assertCountEqual(actual, expected)
                self.assertTrue(all('whole' not in target for target in expected))


if __name__ == '__main__':
    unittest.main()
