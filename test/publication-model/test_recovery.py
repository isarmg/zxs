"""Recovery windows, to be executed by the isolated GitHub verification carrier."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools/publication-model"))
from model import Config, initial, successors


def take(state, config, label):
    for action, target in successors(state, config):
        if action == label:
            return target
    raise AssertionError(f"disabled action {label!r} in {state.phase}")


def complete_member(state, config, phase, member):
    for step in ("write", "file_sync", "install", "directory_sync"):
        state = take(state, config, f"{phase}:{member}:{step}")
    return state


def prepare(state, config):
    for phase, count in (("CONTENT", config.replicas), ("PREPARE", config.n)):
        for member in range(count):
            state = complete_member(state, config, phase, member)
        state = take(state, config, f"barrier:{phase}")
    return take(state, config, "issue_accept")


def advance_publication(state, config):
    """Follow only productive persistence/barrier actions, bounded by 200 steps."""
    for _ in range(200):
        if state.phase == "ACK":
            return state
        for action, target in successors(state, config):
            if (":error:" not in action and
                    (action.startswith(state.phase + ":") or
                     action.startswith("barrier:") or
                     action.startswith("recover:skip_unavailable_") or
                     action == "publish:atomic_state_receipt_event")):
                state = target
                break
        else:
            raise AssertionError(f"no productive action in {state.phase}")
    raise AssertionError("publication exceeded fixed directed-test step budget")


class RecoveryWindows(unittest.TestCase):
    def test_abort_q_without_missing_member_and_no_resurrection(self):
        config = Config(n=3, operations=2, crashes=2, fault_budget=1)
        state = prepare(initial(config), config)
        state = complete_member(state, config, "ACCEPT", 0)
        state = take(state, config, "disk:offline:0")
        state = take(state, config, "crash:process")
        for member in (1, 2):
            state = take(state, config, f"observe:{member}:0")
        state = take(state, config, "recover:abort_evidence")
        for member in (1, 2):
            state = complete_member(state, config, "RECOVER_ABORT", member)
        state = take(state, config, "recover:persist_abort_resolution")
        self.assertEqual(state.results[0], 3)
        self.assertNotIn("next_operation", [a for a, _ in successors(state, config)])
        original_resolution = state.abort_resolution[0]
        state = take(state, config, "disk:return:0")
        state = take(state, config, "crash:process")
        state = take(state, config, "observe:0:1")
        state = take(state, config, "observe:1:4")
        state = take(state, config, "recover:preserve_abort_resolution")
        state = complete_member(state, config, "RECOVER_ABORT", 0)
        state = take(state, config, "recover:persist_abort_resolution")
        self.assertEqual(state.results[0], 3)
        self.assertEqual(state.abort_resolution[0], original_resolution)
        self.assertEqual(state.publish_count[0], 0)

    def test_second_power_failure_during_abort_resolution(self):
        config = Config(n=1, crashes=2)
        state = take(initial(config), config, "crash:process")
        state = take(state, config, "observe:0:0")
        state = take(state, config, "recover:abort_evidence")
        state = take(state, config, "RECOVER_ABORT:0:write")
        state = take(state, config, "crash:power:none:bytes:0")
        self.assertEqual(state.results, (0,))
        self.assertEqual(state.artifacts[0][0][7].data, 0)
        state = take(state, config, "observe:0:0")
        state = take(state, config, "recover:abort_evidence")
        state = complete_member(state, config, "RECOVER_ABORT", 0)
        state = take(state, config, "recover:persist_abort_resolution")
        self.assertEqual(state.results, (3,))

    def test_single_publication_then_damage_keeps_original_normal_result(self):
        config = Config(n=1, crashes=1, fault_budget=1)
        state = prepare(initial(config), config)
        for phase in ("ACCEPT", "FINALIZE", "COMMIT_PROOF", "PUBLICATION"):
            state = complete_member(state, config, phase, 0)
            state = take(state, config, f"barrier:{phase}")
        self.assertEqual(state.results, (0,))
        state = take(state, config, "content:corrupt:0:0")
        state = take(state, config, "crash:process")
        state = take(state, config, "observe:0:2")
        state = take(state, config, "recover:preserve_candidate")
        state = take(state, config, "recover:reprotect_fixed_members")
        state = advance_publication(state, config)
        self.assertEqual(state.results, (1,))
        self.assertEqual(state.artifacts[0][0][0].data, 3)
        self.assertTrue(state.publication_witness[0][0] & 1)

    def test_planned_normal_recovery_does_not_hide_new_content_damage(self):
        config = Config(n=1, crashes=1, fault_budget=1)
        state = prepare(initial(config), config)
        state = complete_member(state, config, "ACCEPT", 0)
        state = take(state, config, "crash:process")
        state = take(state, config, "observe:0:1")
        state = take(state, config, "recover:preserve_candidate")
        state = take(state, config, "recover:reprotect_fixed_members")
        self.assertEqual(state.recovery_result, 1)
        self.assertFalse(state.normal_publication_seen)
        state = take(state, config, "barrier:CONTENT")
        state = take(state, config, "content:corrupt:0:0")
        state = take(state, config, "barrier:PREPARE")
        state = take(state, config, "barrier:ACCEPT")
        state = complete_member(state, config, "FINALIZE", 0)
        state = take(state, config, "barrier:FINALIZE")
        enabled = [action for action, _ in successors(state, config)]
        self.assertIn("recover:known_content_damage", enabled)
        self.assertFalse(any(action.startswith("COMMIT_PROOF:") for action in enabled))
        state = take(state, config, "recover:known_content_damage")
        self.assertEqual(state.results, (0,))
        self.assertEqual(state.phase, "RECOVER_OBSERVE")

    def test_one_healthy_copy_repairs_second_required_copy_before_normal_publication(self):
        config = Config(n=2, replicas=2, crashes=1, fault_budget=1)
        state = prepare(initial(config), config)
        for member in range(2):
            state = complete_member(state, config, "ACCEPT", member)
        state = take(state, config, "content:corrupt:0:0")
        state = take(state, config, "crash:process")
        for member in range(2):
            state = take(state, config, f"observe:{member}:1")
        state = take(state, config, "recover:preserve_candidate")
        state = take(state, config, "recover:reprotect_fixed_members")
        self.assertEqual(state.phase, "CONTENT")
        state = advance_publication(state, config)
        self.assertEqual(state.results, (1,))
        self.assertEqual([state.artifacts[0][m][0].data for m in range(2)], [1, 1])
        self.assertEqual(state.publish_count, (1,))

    def test_unpublished_decision_damage_publishes_explicit_degraded_result(self):
        config = Config(n=1, crashes=1, fault_budget=1)
        state = prepare(initial(config), config)
        state = complete_member(state, config, "ACCEPT", 0)
        state = take(state, config, "content:corrupt:0:0")
        state = take(state, config, "crash:process")
        state = take(state, config, "observe:0:1")
        state = take(state, config, "recover:preserve_candidate")
        state = take(state, config, "recover:reprotect_fixed_members")
        state = advance_publication(state, config)
        self.assertEqual(state.results, (2,))
        self.assertEqual(state.publication_result, (2,))
        self.assertEqual(state.publish_count, (1,))


if __name__ == "__main__":
    unittest.main()
