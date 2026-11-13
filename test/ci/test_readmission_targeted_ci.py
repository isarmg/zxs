"""Preserve readmission cut coverage while changing one call-local proof scope."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'readmission_targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class ReadmissionTargetedCi(unittest.TestCase):
    def test_member_proof_change_keeps_old_groups_and_all_cut_consumers(self):
        policy = json.loads((ROOT / CI.PLAN_PATH).read_text())
        group = 'readmission-member-proof-scope'
        plan = CI.select_plan(policy, [
            'src/engine/src/engine/membership_transition/readmission/core.rs',
            'test/engine/unit/engine/readmission_core_tests.rs',
        ], shardable=True)
        self.assertEqual(plan['groups'], ['held-directory-layout', group])
        targets = policy['groups'][group]['rust_targets']
        engine = targets[1]
        self.assertEqual(engine['features'], ['fault-injection'])
        self.assertEqual(engine['prefixes'], [
            'engine::membership_transition::readmission::',
            'engine::membership_transition::pending::activation::recovery::restricted::',
        ])
        self.assertEqual(engine['max_tests'], 60)
        self.assertEqual(engine['timeout_seconds'], 300)
        self.assertTrue(all(target.get('timeout_seconds', 300) == 300
                            for target in targets))
        self.assertEqual(targets[2]['prefixes'], [
            'management::storage_readmission::tests::',
            'management::storage_readmission::exclusive::tests::',
            'management::terminal_readmission::tests::',
        ])
        self.assertEqual(targets[3]['prefixes'], ['offline::disk_readmission::tests::'])
        old = policy['groups']['held-directory-layout']['rust_targets'][1]
        self.assertEqual(old['prefixes'], ['engine::membership_transition::readmission::'])
        self.assertEqual(old['max_tests'], 21)
        self.assertEqual(old.get('timeout_seconds', 300), 300)


if __name__ == '__main__':
    unittest.main()
