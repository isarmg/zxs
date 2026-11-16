"""Verify the mapping for the shared named-root admission and its physical regressions."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py',
)
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class NamedBackupRootSelection(unittest.TestCase):
    def test_transition_root_and_recheck_schedule_both_physical_gates(self):
        policy = json.loads((ROOT / '.github/ci/targeted-tests.json').read_text())
        empty = 'ordinary-empty-directory-backup'
        named = 'named-backup-root-admission'
        cases = [
            ('src/engine/src/engine/membership_transition.rs', {empty, named}),
            ('src/engine/src/engine/fresh_layout.rs', {empty, named}),
            ('test/engine/unit/disk_names/backup_source_tests.rs', {named}),
            ('test/ci/test_named_backup_root_targeted_ci.py', {empty, named}),
        ]
        for path, required in cases:
            with self.subTest(path=path):
                plan = CI.select_plan(policy, [path], shardable=True)
                self.assertLessEqual(required, set(plan['groups']))
                self.assertIn('test_named_backup_root_targeted_ci', plan['python_tests'])
                for group in required:
                    selected = []
                    for target in plan['rust_targets']:
                        selections = target.get('selections', [{
                            key: value for key, value in target.items()
                            if key != 'selection_groups'
                        }])
                        selected.extend(
                            selection for origin, selection in zip(
                                target['selection_groups'], selections,
                            ) if origin == group
                        )
                    self.assertCountEqual(selected, policy['groups'][group]['rust_targets'])
