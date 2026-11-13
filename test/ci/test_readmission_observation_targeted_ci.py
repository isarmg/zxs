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


class ReadmissionObservationTargetedCi(unittest.TestCase):
    def test_fixture_observation_is_one_original_test_without_shrinking_production(self):
        policy = json.loads((ROOT / CI.PLAN_PATH).read_text())
        plan = CI.select_plan(policy, ['test/engine/unit/engine/exit_stop_probe_tests.rs'])
        self.assertEqual(plan['groups'], ['readmission-exit-stop-observation'])
        self.assertEqual(len(plan['rust_targets']), 1)
        target = plan['rust_targets'][0]
        self.assertEqual(target['features'], ['fault-injection'])
        self.assertEqual(target['exact'], [
            'engine::membership_transition::pending::activation::recovery::restricted::'
            'exit_stop::tests::prepared_exit_stop_uses_readmitted_survivors_at_every_locator_barrier',
        ])
        self.assertEqual((target['max_tests'], target['timeout_seconds']), (1, 300))
        production = CI.select_plan(policy, [
            'src/engine/src/engine/membership_transition/readmission/core.rs',
        ], shardable=True)
        self.assertEqual(production['groups'], [
            'held-directory-layout', 'readmission-member-proof-scope',
        ])


if __name__ == "__main__":
    unittest.main()
