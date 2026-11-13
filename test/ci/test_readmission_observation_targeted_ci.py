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
    def test_four_real_barriers_have_independent_budgets_without_shrinking_production(self):
        policy = json.loads((ROOT / CI.PLAN_PATH).read_text())
        plan = CI.select_plan(policy, ['test/engine/unit/engine/exit_stop_probe_tests.rs'])
        self.assertEqual(plan['groups'], ['readmission-exit-stop-observation'])
        self.assertEqual(len(plan['rust_targets']), 1)
        target = plan['rust_targets'][0]
        self.assertEqual(target['features'], ['fault-injection'])
        prefix = ('engine::membership_transition::pending::activation::recovery::restricted::'
                  'exit_stop::tests::prepared_exit_stop_uses_readmitted_survivors_')
        names = [prefix + suffix for suffix in (
            'before_record', 'after_record', 'before_head', 'after_head')]
        self.assertEqual(target['exact'], names)
        self.assertEqual((target['max_tests'], target['timeout_seconds']), (4, 150))
        self.assertEqual(len(names) * target['timeout_seconds'], 600)
        production = CI.select_plan(policy, [
            'src/engine/src/engine/membership_transition/readmission/core.rs',
        ], shardable=True)
        self.assertEqual(production['groups'], [
            'held-directory-layout', 'readmission-member-proof-scope',
        ])
        selections = policy['groups']['readmission-member-proof-scope']['rust_targets']
        self.assertEqual(selections[-1], policy['groups']['readmission-exit-stop-observation']['rust_targets'][0])
        timeouts = CI.test_timeouts([
            {'configuration': selections[1], 'selected': names},
            {'configuration': selections[-1], 'selected': names},
        ])
        self.assertEqual(timeouts, {name: 150 for name in names})
        original = policy['groups']['held-directory-layout']['rust_targets'][1]
        self.assertEqual(original['max_tests'], 21)
        self.assertEqual(original.get('timeout_seconds', 300), 300)


if __name__ == "__main__":
    unittest.main()
