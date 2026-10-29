"""见证构造的独立负测试：掩码不能代替实际记录和先前观察。

Fixtures state explicit protocol facts. They never ask transition helpers to
generate masks or use the property checker to classify the expected input.
"""
from dataclasses import replace
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools/publication-model"))
from model import ACCEPT, FINALIZE, PREPARE, PUBLICATION, RECOVERY_RESOLUTION, Artifact, Config, initial
from properties import check


def record(state, member, kind, artifact):
    rows = [list(row) for row in state.artifacts[0]]
    rows[member][kind] = artifact
    return replace(state, artifacts=(tuple(tuple(row) for row in rows),))


def complete_record():
    return Artifact(data=1, stable_data=1, file_synced=True, entry=True,
                    dir_synced=True, done=True)


class WitnessEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.config = Config(n=3)

    def assertClean(self, previous, current):
        self.assertEqual(check(previous, current, self.config), ())

    def assertNamed(self, previous, current, name, detail=None):
        found = check(previous, current, self.config)
        named = [item for item in found if item["name"] == name]
        self.assertTrue(named, f"missing {name}: {found}")
        self.assertTrue(all(item["spec"] for item in named))
        if detail is not None:
            self.assertTrue(any(detail in item["message"] for item in named), named)

    def decision_pair(self):
        previous = replace(initial(self.config), phase="ACCEPT", issued=(True,), binding=(1,))
        previous = record(previous, 0, ACCEPT, complete_record())
        current = record(previous, 1, ACCEPT, complete_record())
        return previous, replace(current, decision_witness=(0b011,))

    def absence_pair(self, observations=(0, 0, -1), mask=0b011):
        previous = replace(initial(self.config), phase="RECOVER_OBSERVE", recovery_mode=True,
                           observations=observations)
        return previous, replace(previous, phase="RECOVER_ABORT", abort_witness=(mask,))

    def resolution_pair(self):
        _, previous = self.absence_pair()
        previous = record(previous, 0, RECOVERY_RESOLUTION, complete_record())
        previous = record(previous, 1, RECOVERY_RESOLUTION, complete_record())
        current = replace(previous, abort_resolution=(0b011,), results=(3,),
                          binding=(1,), phase="RESOLVED", response="aborted")
        return previous, current

    def test_real_majority_accepts_establish_decision(self):
        self.assertClean(*self.decision_pair())

    def test_full_quorum_mask_cannot_forge_missing_accept(self):
        previous, current = self.decision_pair()
        current = record(current, 1, ACCEPT, Artifact())
        self.assertNamed(previous, current, "decision_evidence", "member 1 ACCEPT")

    def test_each_accept_durability_fact_is_required(self):
        previous, current = self.decision_pair()
        for field, value in (("data", 2), ("stable_data", 0), ("entry", False), ("dir_synced", False)):
            with self.subTest(field=field):
                altered = record(current, 1, ACCEPT, replace(complete_record(), **{field: value}))
                self.assertNamed(previous, altered, "decision_evidence", f"{field}={value}")

    def test_error_return_can_still_have_real_durable_accept(self):
        previous, current = self.decision_pair()
        current = record(current, 1, ACCEPT, replace(complete_record(), file_synced=False,
                                                   failed=True, done=False))
        self.assertClean(previous, current)

    def test_decision_mask_must_match_complete_physical_set(self):
        previous, current = self.decision_pair()
        current = record(current, 2, ACCEPT, complete_record())
        self.assertNamed(previous, current, "decision_evidence", "physical mask 0b111")
        self.assertClean(previous, replace(current, decision_witness=(0b111,)))

    def test_nonprefix_member_sets_are_valid_witnesses(self):
        previous, current = self.decision_pair()
        current = record(record(current, 1, ACCEPT, Artifact()), 2, ACCEPT, complete_record())
        self.assertClean(previous, replace(current, decision_witness=(0b101,)))
        self.assertClean(*self.absence_pair((0, -1, 0), 0b101))

    def test_out_of_configuration_members_cannot_complete_a_quorum(self):
        previous, current = self.decision_pair()
        self.assertNamed(previous, replace(current, decision_witness=(0b1001,)), "decision_evidence")
        previous, current = self.absence_pair()
        self.assertNamed(previous, replace(current, abort_witness=(0b1001,)), "abort_evidence")
        previous, current = self.resolution_pair()
        self.assertNamed(previous, replace(current, abort_resolution=(0b1001,)), "abort_resolution_evidence")

    def test_complete_absence_snapshot_establishes_abort_witness(self):
        previous, current = self.absence_pair()
        previous = record(previous, 0, PREPARE, complete_record())
        current = record(current, 0, PREPARE, complete_record())
        self.assertClean(previous, current)

    def test_full_quorum_abort_mask_cannot_forge_an_unobserved_member(self):
        self.assertNamed(*self.absence_pair((0, -1, -1)), "abort_evidence", "observations 0b1")

    def test_stale_absence_labels_cannot_hide_present_or_broken_logs(self):
        for kind, artifact in ((ACCEPT, complete_record()), (FINALIZE, complete_record()),
                               (PUBLICATION, complete_record()), (RECOVERY_RESOLUTION, complete_record()),
                               (PREPARE, replace(complete_record(), data=2))):
            with self.subTest(kind=kind):
                previous, current = self.absence_pair()
                previous = record(previous, 1, kind, artifact)
                current = record(current, 1, kind, artifact)
                self.assertNamed(previous, current, "abort_evidence", "member 1")

    def test_abort_cannot_ignore_an_already_observed_candidate(self):
        previous, current = self.absence_pair((0, 0, 1))
        previous = record(previous, 2, ACCEPT, complete_record())
        current = record(current, 2, ACCEPT, complete_record())
        self.assertNamed(previous, current, "abort_evidence", "ignored an observed ACCEPT")

    def test_offline_after_complete_observation_does_not_erase_snapshot(self):
        previous, current = self.absence_pair()
        previous = replace(previous, online=(False, True, True), read_fault=(True, False, False))
        current = replace(current, online=previous.online, read_fault=previous.read_fault)
        self.assertClean(previous, current)

    def test_real_resolution_records_and_prior_absence_establish_abort(self):
        self.assertClean(*self.resolution_pair())

    def test_full_quorum_resolution_mask_cannot_forge_durable_records(self):
        previous, current = self.resolution_pair()
        missing = record(current, 1, RECOVERY_RESOLUTION, Artifact())
        self.assertNamed(previous, missing, "abort_resolution_evidence",
                         "member 1 RECOVERY_RESOLUTION")
        for field, value in (("data", 3), ("stable_data", 0), ("entry", False), ("dir_synced", False)):
            with self.subTest(field=field):
                altered = record(current, 1, RECOVERY_RESOLUTION,
                                 replace(complete_record(), **{field: value}))
                self.assertNamed(previous, altered, "abort_resolution_evidence",
                                 f"member 1 RECOVERY_RESOLUTION")

    def test_resolution_requires_previously_established_absence(self):
        previous, current = self.resolution_pair()
        previous = replace(previous, abort_witness=(0,))
        self.assertNamed(previous, current, "abort_resolution_evidence", "previously established")

    def test_resolution_after_second_crash_keeps_original_absence_proof(self):
        previous, current = self.resolution_pair()
        # The original complete-absence evidence predates resolution writes.
        # A later recovery reads resolution records, so observations are now 4.
        previous = replace(previous, observations=(4, 4, -1))
        current = replace(current, observations=previous.observations)
        self.assertClean(previous, current)

    def test_historical_resolution_mask_cannot_change(self):
        _, previous = self.resolution_pair()
        current = record(previous, 2, RECOVERY_RESOLUTION, complete_record())
        self.assertNamed(previous, replace(current, abort_resolution=(0b111,)),
                         "abort_resolution_evidence", "historical")

    def test_later_damage_does_not_revoke_a_historical_resolution(self):
        _, previous = self.resolution_pair()
        current = record(previous, 1, RECOVERY_RESOLUTION,
                         replace(complete_record(), data=3, stable_data=3, file_synced=False))
        self.assertClean(previous, current)


if __name__ == "__main__":
    unittest.main()
