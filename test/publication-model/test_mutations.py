"""Mutation sensitivity tests, executed by the isolated GitHub validation job.

These tests use only the abstract transition graph, never the product recovery
implementation. A truncated search is a test failure rather than a green result.
"""
from copy import deepcopy
from dataclasses import replace
from pathlib import Path
import sys
import unittest

TOOL = Path(__file__).resolve().parents[2] / "tools" / "publication-model"
sys.path.insert(0, str(TOOL))

from model import Artifact, Config, CORRUPT, PREPARE, initial
import properties
from search import Limits, replay, search


class MutationSensitivityTests(unittest.TestCase):
    def find_and_replay(self, config):
        outcome = search(config, Limits(max_states=100000,
                                        max_transitions=1000000,
                                        max_depth=100,
                                        max_seconds=90))
        self.assertEqual(outcome["status"], "Counterexample",
                         f"{config.variant}: {outcome['status']}, "
                         f"{outcome['truncation']}, {outcome['counts']}")
        self.assertTrue(outcome["violations"], "counterexample needs a named property")
        expected = {
            "shrunken_quorum": {"fixed_quorum"},
            "early_ack": {"publication_barrier"},
            "omit_sync": {"durability_barrier", "fixed_quorum", "publication_barrier"},
            "timeout_rollback": {"no_timeout_abort"},
            "lost_binding": {"unique_publication"},
        }
        self.assertTrue(expected[config.variant].intersection(
            violation["name"] for violation in outcome["violations"]),
            "mutation must expose its intended safety rule")
        for violation in outcome["violations"]:
            self.assertTrue(violation["name"])
            self.assertTrue(violation["spec"])
        trace = outcome["trace"]
        self.assertTrue(trace["steps"], "mutation must be reached by an actual action")
        rerun = replay(trace)
        self.assertTrue(rerun["valid"])
        self.assertTrue(rerun["counterexample"])
        self.assertEqual(rerun["violations"], outcome["violations"])
        self.assertEqual(rerun["final_digest"], trace["steps"][-1]["state_digest"])
        self.assertLessEqual(len(trace["steps"]), outcome["limits"]["max_depth"])
        baseline_trace = deepcopy(trace)
        baseline_trace["config"]["variant"] = "correct"
        with self.assertRaisesRegex(ValueError, "not enabled|state digest mismatch"):
            replay(baseline_trace)
        return outcome

    def test_shrunken_quorum(self):
        self.find_and_replay(Config(n=2, fault_budget=1, variant="shrunken_quorum"))

    def test_early_ack(self):
        self.find_and_replay(Config(n=1, variant="early_ack"))

    def test_omit_sync(self):
        self.find_and_replay(Config(n=1, variant="omit_sync"))

    def test_timeout_rollback(self):
        self.find_and_replay(Config(n=1, variant="timeout_rollback"))

    def test_lost_idempotency_binding(self):
        self.find_and_replay(Config(n=1, variant="lost_binding"))

    def test_saved_mutation_trace_rejects_state_digest_tampering(self):
        outcome = self.find_and_replay(Config(n=1, variant="early_ack"))
        trace = deepcopy(outcome["trace"])
        trace["steps"][-1]["state_digest"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "state digest mismatch"):
            replay(trace)

    def test_saved_mutation_trace_rejects_action_tampering(self):
        outcome = self.find_and_replay(Config(n=1, variant="timeout_rollback"))
        trace = deepcopy(outcome["trace"])
        trace["steps"][-1]["action"] = "not-a-model-transition"
        with self.assertRaisesRegex(ValueError, "action is not enabled"):
            replay(trace)


class OracleSensitivityTests(unittest.TestCase):
    """Synthetic states test the oracle; they are not reachable counterexamples."""

    def test_corrupt_installed_record_is_not_complete_absence(self):
        config = Config(n=1)
        before = initial(config)
        row = list(before.artifacts[0][0])
        row[PREPARE] = Artifact(data=CORRUPT, entry=True)
        damaged = replace(before, artifacts=((tuple(row),),))
        observed = replace(damaged, observations=(0,))
        names = {violation["name"] for violation in properties.check(damaged, observed, config)}
        self.assertIn("recovery_observation", names)

    def test_abort_needs_durable_fixed_quorum_resolution(self):
        config = Config(n=2)
        before = initial(config)
        after = replace(before, results=(3,), abort_witness=(3,), abort_resolution=(1,))
        names = {violation["name"] for violation in properties.check(before, after, config)}
        self.assertIn("abort_resolution", names)

    def test_fabricated_publication_origin_is_rejected(self):
        config = Config(n=1)
        before = initial(config)
        after = replace(before, publication_origin=((127,),))
        names = {violation["name"] for violation in properties.check(before, after, config)}
        self.assertIn("publication_evidence", names)


if __name__ == "__main__":
    unittest.main()
