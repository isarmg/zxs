"""Local suffix searches remain distinct from exhaustive root exploration."""
from contextlib import ExitStack
from dataclasses import dataclass
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools/publication-model"))

import deep_search as engine
import model
import search as root_search


@dataclass(frozen=True)
class TinyConfig:
    n: int = 1


@dataclass(frozen=True)
class TinyState:
    value: int
    phase: str = "SYNTHETIC"
    crash_count: int = 0
    fault_count: int = 0


class DeepSearchTests(unittest.TestCase):
    def graph(self, adjacency, bad_edges=()):
        stack = ExitStack()
        self.addCleanup(stack.close)
        stack.enter_context(patch.object(engine.model, "Config", TinyConfig))
        stack.enter_context(patch.object(engine.model, "initial", lambda config: TinyState(0)))
        stack.enter_context(patch.object(engine.model, "state_to_dict",
                                        lambda state: {"value": state.value, "phase": state.phase}))
        stack.enter_context(patch.object(engine.model, "successors",
            lambda state, config: ((label, TinyState(target))
                                   for label, target in adjacency.get(state.value, ()))))
        bad = set(bad_edges)
        stack.enter_context(patch.object(engine.properties, "check",
            lambda previous, state, config:
                ({"name": "synthetic_edge", "message": "bad edge", "spec": "synthetic graph"},)
                if (None if previous is None else previous.value, state.value) in bad else ()))

    def test_local_exhaustion_is_not_root_exhaustion(self):
        self.graph({0: (("seed", 1), ("unselected", 9)),
                    1: (("forward", 2),), 2: (("back", 1),)})
        result = engine.explore_suffix(TinyConfig(), ["seed"])
        self.assertEqual(result["status"], "LocalExhaustive")
        self.assertEqual(result["scope"], "selected-prefix-suffix")
        self.assertEqual(result["prefix"]["length"], 1)
        self.assertEqual(result["counts"]["states"], 2)
        self.assertNotEqual(result["status"], "BoundedExhaustive")

    def test_counterexample_trace_contains_prefix_and_suffix(self):
        self.graph({0: (("prefix-a", 1),), 1: (("prefix-b", 2),),
                    2: (("unsafe", 3),)}, ((2, 3),))
        result = engine.explore_suffix(TinyConfig(), ["prefix-a", "prefix-b"],
                                      root_search.Limits(max_depth=1))
        self.assertEqual(result["status"], "Counterexample")
        trace = result["trace"]
        self.assertEqual([entry["action"] for entry in trace["steps"]],
                         ["prefix-a", "prefix-b", "unsafe"])
        self.assertEqual(result["prefix"]["length"], 2)
        self.assertEqual(result["counts"]["max_depth"], 1)
        replay = root_search.replay(trace)
        self.assertTrue(replay["valid"])
        self.assertTrue(replay["counterexample"])
        self.assertEqual(replay["first_violation_step"], 3)

    def test_every_prefix_edge_is_checked(self):
        self.graph({0: (("unsafe-prefix", 1),)}, ((0, 1),))
        with self.assertRaises(ValueError):
            engine.explore_suffix(TinyConfig(), ["unsafe-prefix"])

    def test_disabled_prefix_is_rejected(self):
        self.graph({0: (("actual", 1),)})
        with self.assertRaises(ValueError):
            engine.explore_suffix(TinyConfig(), ["not-enabled"])

    def test_prefix_step_budget_is_inconclusive(self):
        self.graph({0: (("a", 1),), 1: (("b", 2),)})
        result = engine.explore_suffix(TinyConfig(), ["a", "b"], max_prefix_steps=1)
        self.assertEqual(result["status"], "Inconclusive")
        self.assertIn("prefix", result["truncation"])

    def test_prefix_successor_budget_is_inconclusive(self):
        self.graph({0: (("first", 1), ("second", 2))})
        result = engine.explore_suffix(TinyConfig(), ["second"], max_prefix_successors=1)
        self.assertEqual(result["status"], "Inconclusive")
        self.assertIn("prefix", result["truncation"])

    def test_suffix_depth_boundary_does_not_report_out_of_bounds_violation(self):
        self.graph({0: (("seed", 1),), 1: (("too-deep", 2),)}, ((1, 2),))
        result = engine.explore_suffix(TinyConfig(), ["seed"],
                                      root_search.Limits(max_depth=0))
        self.assertEqual(result["status"], "Inconclusive")
        self.assertEqual(result["truncation"], "max_depth")
        self.assertIsNone(result["trace"])

    def test_suffix_checks_edges_to_already_seen_states(self):
        self.graph({0: (("seed", 1),), 1: (("left", 2), ("right", 3)),
                    3: (("bad-backedge", 2),)}, ((3, 2),))
        result = engine.explore_suffix(TinyConfig(), ["seed"])
        self.assertEqual(result["status"], "Counterexample")
        self.assertTrue(root_search.replay(result["trace"])["counterexample"])

    def test_suffix_resource_cutoffs_are_not_local_passes(self):
        self.graph({0: (("seed", 1),), 1: (("a", 2),), 2: (("b", 3),)})
        for limits, reason in ((root_search.Limits(max_states=1), "max_states"),
                               (root_search.Limits(max_transitions=1), "max_transitions")):
            with self.subTest(reason=reason):
                result = engine.explore_suffix(TinyConfig(), ["seed"], limits)
                self.assertEqual(result["status"], "Inconclusive")
                self.assertEqual(result["truncation"], reason)

    def test_duplicate_prefix_actions_are_model_errors(self):
        self.graph({0: (("same", 1), ("same", 2))})
        with self.assertRaises(ValueError):
            engine.explore_suffix(TinyConfig(), ["same"])

    def test_time_budget_applies_to_prefix_and_suffix(self):
        self.graph({0: (("seed", 1),), 1: (("next", 2),)})
        for actions, reason in ((["seed"], "max_seconds_during_prefix"),
                                ([], "max_seconds")):
            with self.subTest(reason=reason), patch.object(
                    engine.time, "monotonic", side_effect=[0.0] + [2.0] * 20):
                result = engine.explore_suffix(TinyConfig(), actions,
                                              root_search.Limits(max_seconds=1))
                self.assertEqual(result["status"], "Inconclusive")
                self.assertEqual(result["truncation"], reason)

    def test_invalid_prefix_budgets_are_rejected(self):
        self.graph({})
        for options in ({"max_prefix_steps": -1}, {"max_prefix_successors": 0}):
            with self.subTest(options=options), self.assertRaises(ValueError):
                engine.explore_suffix(TinyConfig(), [], **options)

    def test_real_prefix_preserves_consumed_fault_and_crash_counts(self):
        config = model.Config(n=1, crashes=1, fault_budget=1)
        actions = ["CONTENT:0:write:error:2", "crash:process"]
        state = model.initial(config)
        for action in actions:
            before = state
            state = dict(model.successors(state, config))[action]
            self.assertFalse(engine.properties.check(before, state, config))
        self.assertEqual((state.fault_count, state.crash_count), (1, 1))
        result = engine.explore_suffix(config, actions, root_search.Limits(max_depth=0))
        self.assertEqual(result["status"], "Inconclusive")
        self.assertEqual(result["prefix"]["end_digest"], root_search.state_digest(state))
        prefix_replay = root_search.replay(result["prefix"]["trace"])
        self.assertTrue(prefix_replay["valid"])
        self.assertFalse(prefix_replay["counterexample"])


if __name__ == "__main__":
    unittest.main()
