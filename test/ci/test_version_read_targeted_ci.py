"""Keep version-read coverage additive to the existing overlapping selections."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'version_read_targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)

PATHS = [
    '.github/ci/targeted-tests.json',
    'src/engine/src/engine/read_view.rs',
    'src/server/src/engine_backend.rs',
    'test/engine/unit/engine/read_view_tests.rs',
    'test/server/unit/engine_backend/version_read_tests.rs',
    'test/ci/test_version_read_targeted_ci.py',
]


class VersionReadSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_complete_change_keeps_original_overlaps_and_build_limit(self):
        plan = CI.select_plan(self.policy, PATHS)
        self.assertEqual(plan['groups'], [
            'ci-selection', 'note-http-boundary', 'paged-deletion-review',
            'version-metadata-read'])
        self.assertEqual(len(plan['rust_targets']), 9)
        self.assertEqual(len(plan['python_tests']), 8)
        self.assertIn('test_version_read_targeted_ci', plan['python_tests'])
        expected = {
            ('lib', None): {
                'note-http-boundary': 14,
                'paged-deletion-review': 80,
                'version-metadata-read': 7,
            },
            ('test', 'http_engine'): {
                'note-http-boundary': 10,
                'version-metadata-read': 3,
            },
            ('test', 'phase1_smb_rpc'): {
                'paged-deletion-review': 80,
                'version-metadata-read': 2,
            },
        }
        for target in plan['rust_targets']:
            if (target['package'] != 'storage-server'
                    or target.get('mode', 'test') != 'test'):
                continue
            identity = (target.get('kind', 'lib'), target.get('name'))
            selections = target['selections']
            self.assertEqual(
                dict(zip(target['selection_groups'], [s['max_tests'] for s in selections])),
                expected.pop(identity))
            for group, selection in zip(target['selection_groups'], selections):
                self.assertIn(selection, self.policy['groups'][group]['rust_targets'])
        self.assertEqual(expected, {})

    def test_query_and_all_three_consumer_tests_select_the_real_family(self):
        for path in PATHS[1:]:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('version-metadata-read', plan['groups'])
        group = self.policy['groups']['version-metadata-read']
        engine = [target for target in group['rust_targets']
                  if target['package'] == 'storage-engine']
        self.assertEqual({target.get('mode', 'test') for target in engine}, {'test', 'check'})
        runtime = next(target for target in engine if target.get('mode', 'test') == 'test')
        self.assertEqual(runtime['prefixes'], ['engine::read_view::tests::'])
        self.assertEqual(runtime['max_tests'], 4)
        server = next(target for target in group['rust_targets']
                      if target['package'] == 'storage-server'
                      and target.get('kind', 'lib') == 'lib'
                      and target.get('mode', 'test') == 'test')
        self.assertEqual(server['prefixes'], ['engine_backend::version_read_tests::'])
        self.assertEqual(server['max_tests'], 7)
        self.assertIn(
            'engine_backend::revocation_tests::completed_account_revocation_precedes_all_engine_admissions',
            server['exact'])
