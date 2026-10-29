"""Directed fault windows complement exhaustive exploration of small graphs."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools/publication-model"))
import model
import properties


class ProtocolWindows(unittest.TestCase):
    def step(self, state, config, action):
        choices = dict(model.successors(state, config))
        self.assertIn(action, choices)
        following = choices[action]
        self.assertFalse(properties.check(state, following, config))
        return following

    def stage_member(self, state, config, phase, member):
        for suffix in ("write", "file_sync", "install", "directory_sync"):
            state = self.step(state, config, f"{phase}:{member}:{suffix}")
        return state

    def drive_to(self, state, config, phase):
        for _ in range(400):
            if state.phase == phase:
                return state
            choices = dict(model.successors(state, config))
            if state.phase in model.STAGES:
                enabled = [a for a in choices if ":error:" not in a and
                           (a.startswith(state.phase + ":") or
                            a.startswith("barrier:") or a == "issue_accept")]
            else:
                enabled = [a for a in choices if a in (
                    "publish:atomic_state_receipt_event", "reply:delivered", "finish")]
            self.assertTrue(enabled, f"no healthy step from {state.phase}")
            state = self.step(state, config, sorted(enabled)[0])
        self.fail(f"did not reach {phase}")

    def test_nonprefix_accept_completion_decides_without_publication(self):
        c = model.Config(n=3)
        s = self.drive_to(model.initial(c), c, "ACCEPT")
        s = self.step(s, c, "issue_accept")
        for member in (2, 0):
            s = self.stage_member(s, c, "ACCEPT", member)
        self.assertEqual(s.decision_witness[0], 0b101)
        self.assertFalse(s.artifacts[0][1][model.ACCEPT].done)
        self.assertNotIn("barrier:ACCEPT", dict(model.successors(s, c)))
        self.assertEqual(s.results, (0,))

    def test_two_sequential_full_publications_for_each_member_count(self):
        for members in (1, 2, 3, 4):
            with self.subTest(members=members):
                c = model.Config(n=members, replicas=min(members, 2), operations=2)
                s = self.drive_to(model.initial(c), c, "ACK")
                s = self.step(s, c, "reply:lost")
                s = self.step(s, c, "retry:original_id")
                s = self.step(s, c, "next_operation")
                s = self.drive_to(s, c, "ACK")
                self.assertEqual(s.results, (1, 1))
                self.assertEqual(s.events, (0, 1))
                self.assertEqual(s.publish_count, (1, 1))

    def test_nonprefix_failures_are_independent_and_keep_fixed_barrier(self):
        c = model.Config(n=4, fault_budget=2)
        s = self.drive_to(model.initial(c), c, "ACCEPT")
        s = self.step(s, c, "issue_accept")
        for member in (2, 0):
            s = self.stage_member(s, c, "ACCEPT", member)
        s = self.step(s, c, "ACCEPT:3:write:error:2")
        s = self.step(s, c, "ACCEPT:1:write:error:3")
        self.assertEqual(s.artifacts[0][3][model.ACCEPT].data, model.TRUNCATED)
        self.assertEqual(s.artifacts[0][1][model.ACCEPT].data, model.CORRUPT)
        self.assertNotIn("barrier:ACCEPT", dict(model.successors(s, c)))

    def test_process_exit_and_power_loss_have_different_byte_outcomes(self):
        c = model.Config(n=1, crashes=1)
        s = self.step(model.initial(c), c, "CONTENT:0:write")
        choices = dict(model.successors(s, c))
        self.assertEqual(choices["crash:process"].artifacts[0][0][0].data, model.FULL)
        power = [v for a, v in choices.items() if a.startswith("crash:power:")]
        self.assertTrue(power)
        self.assertTrue(any(v.artifacts[0][0][0].data != model.FULL for v in power))

    def test_file_sync_error_can_leave_persistent_bytes(self):
        c = model.Config(n=1, fault_budget=1)
        s = self.step(model.initial(c), c, "CONTENT:0:write")
        s = self.step(s, c, "CONTENT:0:file_sync:error:1")
        a = s.artifacts[0][0][0]
        self.assertEqual(a.stable_data, model.FULL)
        self.assertTrue(a.failed)
        self.assertFalse(a.file_synced)
        self.assertFalse(a.dir_synced)

    def test_unsynced_directory_name_can_survive_or_disappear(self):
        c = model.Config(n=1, crashes=1)
        s = model.initial(c)
        for suffix in ("write", "file_sync", "install"):
            s = self.step(s, c, "CONTENT:0:" + suffix)
        power = [v for a, v in model.successors(s, c) if a.startswith("crash:power:")]
        self.assertEqual({v.artifacts[0][0][0].entry for v in power}, {False, True})
        self.assertTrue(all(v.artifacts[0][0][0].stable_data == model.FULL for v in power))

    def test_second_crash_can_interrupt_recovery(self):
        c = model.Config(n=1, crashes=2)
        s = self.drive_to(model.initial(c), c, "ACCEPT")
        s = self.step(s, c, "issue_accept")
        s = self.stage_member(s, c, "ACCEPT", 0)
        s = self.step(s, c, "crash:process")
        s = self.step(s, c, "observe:0:1")
        s = self.step(s, c, "recover:preserve_candidate")
        s = self.step(s, c, "recover:reprotect_fixed_members")
        self.assertTrue(s.recovery_mode)
        s = self.step(s, c, "crash:process")
        self.assertEqual(s.crash_count, 2)
        self.assertEqual(s.phase, "RECOVER_OBSERVE")
        self.assertFalse(any(a.startswith("crash:") for a, _ in model.successors(s, c)))

    def test_single_publication_record_is_not_all_member_completion(self):
        c = model.Config(n=2)
        s = self.drive_to(model.initial(c), c, "PUBLICATION")
        s = self.stage_member(s, c, "PUBLICATION", 0)
        self.assertFalse(s.artifacts[0][1][model.PUBLICATION].done)
        self.assertEqual(s.results, (0,))
        self.assertNotIn("barrier:PUBLICATION", dict(model.successors(s, c)))

    def test_unreadable_log_cannot_supply_absence_evidence(self):
        c = model.Config(n=2, crashes=1, fault_budget=1)
        s = self.step(model.initial(c), c, "disk:unreadable:1")
        s = self.step(s, c, "crash:process")
        s = self.step(s, c, "observe:0:0")
        s = self.step(s, c, "observe:1:3")
        self.assertNotIn("recover:abort_evidence", dict(model.successors(s, c)))

    def test_lost_reply_retry_and_digest_mismatch_preserve_business_effect(self):
        c = model.Config(n=1)
        s = self.drive_to(model.initial(c), c, "ACK")
        s = self.step(s, c, "reply:lost")
        for action in ("retry:original_id", "retry:digest_mismatch"):
            retried = self.step(s, c, action)
            self.assertEqual(retried.results, s.results)
            self.assertEqual(retried.events, (0,))
            self.assertEqual(retried.publish_count, (1,))

    def test_later_content_and_index_failure_preserve_historical_result(self):
        c = model.Config(n=1, fault_budget=2)
        s = self.drive_to(model.initial(c), c, "ACK")
        s = self.step(s, c, "content:corrupt:0:0")
        s = self.step(s, c, "index:fail")
        self.assertEqual(s.results, (1,))
        self.assertEqual(s.events, (0,))
        self.assertTrue(s.index_failed)


if __name__ == "__main__":
    unittest.main()
