"""搜索器边界和反例完整性测试，使用与协议无关的小图。"""
from contextlib import ExitStack
from dataclasses import dataclass
import pathlib
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "tools/publication-model"))
import search as engine


@dataclass(frozen=True)
class TinyConfig:
    n: int = 1


class SearchTests(unittest.TestCase):
    def graph(self, adjacency, bad_edges=()):
        stack = ExitStack()
        self.addCleanup(stack.close)
        stack.enter_context(patch.object(engine.model, "Config", TinyConfig))
        stack.enter_context(patch.object(engine.model, "initial", lambda config: 0))
        stack.enter_context(patch.object(engine.model, "state_to_dict", lambda state: {"v": state}))
        stack.enter_context(patch.object(engine.model, "successors",
                                        lambda state, config: adjacency.get(state, ())))
        bad = set(bad_edges)
        stack.enter_context(patch.object(engine.properties, "check", lambda previous, state, config:
                                        ({"name": "tiny_safety", "message": "bad edge",
                                          "spec": "synthetic graph"},)
                                        if (previous, state) in bad else ()))

    def test_exact_dedup_finishes_cycles(self):
        self.graph({0: (("go", 1),), 1: (("back", 0),)})
        result = engine.search(TinyConfig())
        self.assertEqual(result["status"], "BoundedExhaustive")
        self.assertEqual(result["counts"]["states"], 2)
        self.assertEqual(result["counts"]["transitions"], 2)

    def test_shortest_deterministic_witness_and_replay(self):
        self.graph({0: (("z-long", 1), ("a-short", 2)), 1: (("long", 3),),
                    2: (("bad", 4),), 3: (("bad-later", 4),)}, ((2, 4), (3, 4)))
        result = engine.search(TinyConfig())
        self.assertEqual(result["status"], "Counterexample")
        self.assertEqual([step["action"] for step in result["trace"]["steps"]],
                         ["a-short", "bad"])
        replay = engine.replay(result["trace"])
        self.assertTrue(replay["valid"])
        self.assertTrue(replay["counterexample"])
        self.assertEqual(replay["first_violation_step"], 2)

    def test_edges_to_seen_states_are_checked(self):
        self.graph({0: (("first", 1), ("second", 2)), 2: (("bad-back", 1),)}, ((2, 1),))
        self.assertEqual(engine.search(TinyConfig())["status"], "Counterexample")

    def test_initial_violation_trace_preserves_expected_property(self):
        self.graph({}, ((None, 0),))
        result = engine.search(TinyConfig())
        self.assertEqual(result["status"], "Counterexample")
        self.assertEqual(result["trace"]["steps"], [])
        self.assertEqual(result["trace"]["violations"], result["violations"])
        self.assertTrue(engine.replay(result["trace"])["counterexample"])
        result["trace"]["violations"] = []
        with self.assertRaisesRegex(ValueError, "violation differs"):
            engine.replay(result["trace"])

    def test_depth_cutoff_is_not_a_pass_or_out_of_bounds_violation(self):
        self.graph({0: (("go", 1),), 1: (("too-deep", 2),)}, ((1, 2),))
        result = engine.search(TinyConfig(), engine.Limits(max_depth=1))
        self.assertEqual(result["status"], "Inconclusive")
        self.assertEqual(result["truncation"], "max_depth")
        self.assertIsNone(result["trace"])

    def test_depth_boundary_backedges_are_not_silently_unchecked(self):
        self.graph({0: (("go", 1),), 1: (("back", 0),)})
        self.assertEqual(engine.search(TinyConfig(), engine.Limits(max_depth=1))["status"],
                         "Inconclusive")

    def test_state_and_transition_cutoffs_are_inconclusive(self):
        self.graph({0: (("a", 1), ("b", 2))})
        for limits, reason in ((engine.Limits(max_states=1), "max_states"),
                               (engine.Limits(max_transitions=1), "max_transitions")):
            with self.subTest(reason=reason):
                result = engine.search(TinyConfig(), limits)
                self.assertEqual(result["status"], "Inconclusive")
                self.assertEqual(result["truncation"], reason)

    def test_replay_rejects_changed_digest_and_disabled_action(self):
        self.graph({0: (("go", 1),)}, ((0, 1),))
        trace = engine.search(TinyConfig())["trace"]
        trace["steps"][0]["state_digest"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "state digest mismatch"):
            engine.replay(trace)
        trace["steps"][0]["action"] = "disabled"
        with self.assertRaisesRegex(ValueError, "not enabled"):
            engine.replay(trace)

    def test_duplicate_labels_are_model_errors(self):
        self.graph({0: (("same", 1), ("same", 2))})
        with self.assertRaisesRegex(ValueError, "ambiguous duplicate"):
            engine.search(TinyConfig())

    def test_wide_successors_respect_budget_before_materialization(self):
        self.graph({})
        generated = []

        def wide(state, config):
            for number in range(20):
                generated.append(number)
                yield f"edge-{number}", number + 1

        with patch.object(engine.model, "successors", wide):
            result = engine.search(TinyConfig(), engine.Limits(max_states=1))
        self.assertEqual(result["status"], "Inconclusive")
        self.assertEqual(generated, [0])

    def test_replay_wide_successors_fail_with_explicit_budget(self):
        self.graph({0: (("go", 1),)}, ((0, 1),))
        trace = engine.search(TinyConfig())["trace"]
        with patch.object(engine.model, "successors", lambda state, config:
                          ((f"edge-{number}", number + 1) for number in range(20))):
            with self.assertRaisesRegex(ValueError, "replay successor budget exceeded"):
                engine.replay(trace, max_successors=3)

    def test_invalid_resource_limits_are_rejected(self):
        for fields in ({"max_states": 0}, {"max_transitions": 0}, {"max_depth": -1},
                       {"max_seconds": 0}):
            with self.subTest(fields=fields), self.assertRaises(ValueError):
                engine.Limits(**fields)


if __name__ == "__main__":
    unittest.main()
