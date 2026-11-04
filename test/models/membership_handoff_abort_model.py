"""Pure handoff-abort model: shared single-vote slots and core/witness separation.

No disk, network, Rust build, or subprocess operations. SHA-256 below is only a
model identity oracle, not the production BLAKE3/canonical-JSON implementation.
No claim is made about real filesystem barriers or Rust integration correctness.
"""

from dataclasses import asdict, dataclass, replace
from hashlib import sha256
from itertools import combinations, permutations, product
import json
import unittest


class Rejected(Exception):
    pass


def digest(domain, value):
    payload = json.dumps(value, sort_keys=True, separators=(",", ":"))
    return sha256((domain + "\0" + payload).encode()).hexdigest()


@dataclass(frozen=True, order=True)
class Identity:
    disk: int
    slot: int
    binding: int = 1
    device: int = 7
    inode: int = 0


def old_members(count):
    return tuple(Identity(i, i, inode=100 + i) for i in range(count))


@dataclass(frozen=True)
class Intent:
    old: tuple
    target: tuple
    predecessor: str = "genesis"
    operation: str = "op-original-add"
    boundary: int = 8
    fence: int = 9

    @property
    def hash(self):
        return digest("intent", asdict(self))

    @property
    def quorum(self):
        return len(self.old) // 2 + 1


@dataclass(frozen=True)
class Core:
    intent_hash: str
    archive: str

    @property
    def hash(self):
        # The generation identity has no witnesses, certificate, timestamp,
        # selected voters, online count, or cancellation request nonce.
        return digest("membership-generation", asdict(self))

    def validate(self, intent):
        if self.intent_hash != intent.hash or self.archive != "frozen-exact":
            raise Rejected("wrong frozen intent/archive")


@dataclass(frozen=True)
class Vote:
    intent_hash: str
    member: Identity
    choice: str
    value: str

    @property
    def hash(self):
        return digest("decision", asdict(self))


@dataclass(frozen=True)
class Receipt:
    vote: Vote
    record_hash: str


@dataclass(frozen=True)
class Slot:
    """Visible rename and durable selection differ until the parent barrier.

    Corrupt/torn named records are never represented by an empty slot. A
    successful barrier is assumed not to roll back during later crashes.
    """

    member: Identity
    intent_hash: str
    visible: object = None
    durable: object = None
    corrupt: bool = False

    def publish(self, vote):
        if self.corrupt:
            raise Rejected("corrupt is not vacant")
        if vote.member != self.member or vote.intent_hash != self.intent_hash:
            raise Rejected("held root or intent mismatch")
        if self.visible is not None and self.visible != vote:
            raise Rejected("no-replace decision collision")
        return replace(self, visible=vote)

    def synchronize(self):
        if self.corrupt or self.visible is None:
            raise Rejected("no complete named decision")
        synced = replace(self, durable=self.visible)
        return synced, Receipt(self.visible, self.visible.hash)

    def crash(self, preserve_unsynced):
        if self.corrupt:
            return self
        surviving = self.visible if preserve_unsynced else self.durable
        return replace(self, visible=surviving, durable=surviving)

    def confirms(self, receipt):
        return (
            not self.corrupt
            and self.durable == receipt.vote
            and self.visible == receipt.vote
            and receipt.vote.member == self.member
            and receipt.vote.intent_hash == self.intent_hash
            and receipt.record_hash == receipt.vote.hash
        )


@dataclass(frozen=True)
class Certificate:
    core: Core
    receipts: tuple

    @property
    def hash(self):
        return digest("abort-certificate", asdict(self))


def certify(intent, core, receipts, slots):
    """Online aggregation requires actual synchronized held-member receipts."""
    core.validate(intent)
    ordered = tuple(sorted(receipts, key=lambda receipt: receipt.vote.member))
    members = tuple(receipt.vote.member for receipt in ordered)
    if len(set(members)) != len(members):
        raise Rejected("duplicate voter")
    if not set(members).issubset(intent.old):
        raise Rejected("not an exact original member identity")
    if len(members) < intent.quorum:
        raise Rejected("fixed original quorum unavailable")
    for receipt in ordered:
        vote = receipt.vote
        if vote.choice != "abort" or vote.value != core.hash:
            raise Rejected("another outcome or core")
        slot = slots.get(vote.member)
        if slot is None or not slot.confirms(receipt):
            raise Rejected("receipt lacks actual synchronized member slot")
    return Certificate(core, ordered)


def joint_count(intent, slots):
    return sum(
        slot.durable is not None
        and slot.durable.choice == "joint"
        and slot.member in intent.old
        and slot.intent_hash == intent.hash
        for slot in slots.values()
    )


def joint_admitted(intent, slots):
    # Current ordinary Joint already needs every original control root.
    if any(member not in slots for member in intent.old):
        raise Rejected("cannot scan every original decision")
    if any(slots[member].corrupt for member in intent.old):
        raise Rejected("corrupt original slot")
    if any(
        slots[member].visible is not None
        and slots[member].visible.choice == "abort"
        for member in intent.old
    ):
        raise Rejected("cancellation in progress, even below quorum")


@dataclass(frozen=True)
class Renewal:
    generation: str
    intent: Intent
    effective: tuple
    old_fenced: bool = True
    resources_ready: bool = False
    publication: bool = False
    candidate_bytes: bytes = b"original Candidate bytes"
    archive_bytes: bytes = b"original FrozenArchive bytes"

    @classmethod
    def from_certificate(cls, intent, certificate):
        # Evidence was validated by certify, but is excluded from identity.
        return cls(certificate.core.hash, intent, intent.old)

    def promote(self, held_members, archive_verified):
        # Preserve current activation's all-original-resource contract. This
        # is separate from certification, which only requires a fixed old Q.
        if set(held_members) != set(self.intent.old) or not archive_verified:
            raise Rejected("certificate does not prove all resources ready")
        return replace(self, resources_ready=True)

    def publish(self):
        if not self.resources_ready:
            raise Rejected("cannot publish cancellation before promotion")
        return replace(self, publication=True)

    @property
    def writable(self):
        return self.resources_ready and self.publication

    @property
    def result(self):
        if not self.publication:
            raise Rejected("not a published operation result")
        return (
            self.intent.operation,
            "membership-add-cancelled",
            self.intent.hash,
            self.intent.fence + 1,
            self.generation,
        )


def install_envelope(existing, incoming):
    """Inputs model fully verified envelopes, never raw caller certificates."""
    if existing is None:
        return incoming
    if existing.core.hash != incoming.core.hash:
        raise Rejected("authority conflict")
    # Preserve the existing valid bytes; witness equality is not required.
    return existing


def fixture(count=3):
    old = old_members(count)
    new = Identity(99, 99, inode=999)
    intent = Intent(old, old + (new,))
    slots = {member: Slot(member, intent.hash) for member in old}
    return intent, slots


def vote_on(slots, member, choice, value):
    slot = slots[member]
    vote = Vote(slot.intent_hash, member, choice, value)
    slots[member], receipt = slot.publish(vote).synchronize()
    return receipt


class AbortModelTests(unittest.TestCase):
    def test_fixed_majorities_cannot_joint_and_abort_or_two_abort_cores(self):
        # Exhaust terminal single-slot assignments through N=7, including
        # even N. Unavailable or unknown members contribute no vote.
        for count in range(1, 8):
            quorum = count // 2 + 1
            choices = ("empty", "joint", "abort-a", "abort-b")
            for assignment in product(choices, repeat=count):
                winners = [
                    kind for kind in choices[1:]
                    if assignment.count(kind) >= quorum
                ]
                self.assertLessEqual(len(winners), 1, assignment)

    def test_minority_joint_can_coexist_with_certified_abort(self):
        intent, slots = fixture(5)
        for member in intent.old[:2]:
            vote_on(slots, member, "joint", "proposal")
        core = Core(intent.hash, "frozen-exact")
        receipts = [
            vote_on(slots, member, "abort", core.hash)
            for member in intent.old[2:]
        ]
        certificate = certify(intent, core, receipts, slots)
        self.assertEqual(len(certificate.receipts), intent.quorum)
        self.assertEqual(joint_count(intent, slots), 2)
        for member in intent.old[:2]:
            with self.assertRaises(Rejected):
                slots[member].publish(Vote(intent.hash, member, "abort", core.hash))

    def test_no_joint_quorum_is_insufficient_for_even_membership(self):
        intent, slots = fixture(4)
        for member in intent.old[:2]:
            vote_on(slots, member, "joint", "proposal")
        self.assertLess(joint_count(intent, slots), intent.quorum)
        free = sum(slot.visible is None for slot in slots.values())
        self.assertLess(free, intent.quorum)

    def test_joint_majority_blocks_every_possible_abort_quorum(self):
        intent, slots = fixture(5)
        for member in intent.old[:intent.quorum]:
            vote_on(slots, member, "joint", "proposal")
        for voters in combinations(intent.old, intent.quorum):
            self.assertTrue(any(slots[member].durable is not None for member in voters))

    def test_online_count_cannot_lower_original_quorum(self):
        intent, slots = fixture(5)
        core = Core(intent.hash, "frozen-exact")
        receipts = [vote_on(slots, member, "abort", core.hash) for member in intent.old[:2]]
        online = {member: slots[member] for member in intent.old[:2]}
        with self.assertRaises(Rejected):
            certify(intent, core, receipts, online)

    def test_new_only_relabelled_or_duplicate_identities_do_not_vote(self):
        intent, slots = fixture()
        core = Core(intent.hash, "frozen-exact")
        real = vote_on(slots, intent.old[0], "abort", core.hash)
        wrong_members = (
            intent.target[-1],
            replace(intent.old[1], binding=2),
            replace(intent.old[1], inode=900),
        )
        for member in wrong_members:
            wrong_slot = Slot(member, intent.hash)
            vote = Vote(intent.hash, member, "abort", core.hash)
            wrong_slot, receipt = wrong_slot.publish(vote).synchronize()
            available = dict(slots)
            available[member] = wrong_slot
            with self.assertRaises(Rejected):
                certify(intent, core, [real, receipt], available)
        with self.assertRaises(Rejected):
            certify(intent, core, [real, real], slots)

    def test_arbitrary_witness_subsets_change_certificate_not_generation(self):
        intent, slots = fixture()
        core = Core(intent.hash, "frozen-exact")
        receipts = [vote_on(slots, member, "abort", core.hash) for member in intent.old]
        certificates = [
            certify(intent, core, subset, slots)
            for subset in combinations(receipts, intent.quorum)
        ]
        self.assertGreater(len({certificate.hash for certificate in certificates}), 1)
        good_ids = {
            Renewal.from_certificate(intent, certificate).generation
            for certificate in certificates
        }
        self.assertEqual(good_ids, {core.hash})
        bad_ids = {
            digest("bad-full-authority", {"core": core.hash, "witness": certificate.hash})
            for certificate in certificates
        }
        self.assertGreater(len(bad_ids), 1)
        first = certificates[0]
        for certificate in certificates[1:]:
            self.assertEqual(install_envelope(first, certificate), first)

    def test_receipt_order_is_canonical_but_witness_subset_is_not_fixed(self):
        intent, slots = fixture(5)
        core = Core(intent.hash, "frozen-exact")
        receipts = [vote_on(slots, member, "abort", core.hash) for member in intent.old]
        variants = {
            certify(intent, core, ordering, slots).hash
            for ordering in permutations(receipts[:intent.quorum])
        }
        self.assertEqual(len(variants), 1)
        self.assertNotEqual(
            certify(intent, core, receipts[:intent.quorum], slots).hash,
            certify(intent, core, receipts[-intent.quorum:], slots).hash,
        )

    def test_lost_initial_witness_can_be_replaced_with_another_fixed_old_quorum(self):
        intent, slots = fixture()
        core = Core(intent.hash, "frozen-exact")
        first = vote_on(slots, intent.old[0], "abort", core.hash)
        lost = vote_on(slots, intent.old[1], "abort", core.hash)
        original = certify(intent, core, [first, lost], slots)
        online = {member: slot for member, slot in slots.items() if member != intent.old[1]}
        replacement = vote_on(online, intent.old[2], "abort", core.hash)
        recovered = certify(intent, core, [first, replacement], online)
        self.assertNotEqual(original.hash, recovered.hash)
        self.assertEqual(original.core.hash, recovered.core.hash)
        # A precommitted first voter set would be unavailable here despite Q.
        fixed_voters = {intent.old[0], intent.old[1]}
        self.assertFalse(fixed_voters.issubset(online))
        self.assertEqual(len(online), intent.quorum)

    def test_a_member_cannot_change_abort_core(self):
        intent, slots = fixture()
        member = intent.old[0]
        vote_on(slots, member, "abort", "core-a")
        with self.assertRaises(Rejected):
            vote_on(slots, member, "abort", "core-b")

    def test_atomic_visibility_crash_and_confirmation(self):
        intent, slots = fixture()
        member = intent.old[0]
        joint = Vote(intent.hash, member, "joint", "proposal")
        abort = Vote(intent.hash, member, "abort", "core")
        visible = slots[member].publish(joint)
        with self.assertRaises(Rejected):
            visible.publish(abort)
        # No receipt was emitted; an unsynced name may disappear on crash.
        self.assertEqual(visible.crash(False).publish(abort).visible, abort)
        with self.assertRaises(Rejected):
            visible.crash(True).publish(abort)
        synced, receipt = visible.synchronize()
        for preserve in (False, True):
            after = synced.crash(preserve)
            self.assertTrue(after.confirms(receipt))
            with self.assertRaises(Rejected):
                after.publish(abort)
            repeated, repeated_receipt = after.synchronize()
            self.assertEqual(repeated_receipt, receipt)
            self.assertEqual(repeated, after)

    def test_unsynced_or_fabricated_receipt_cannot_count(self):
        intent, slots = fixture()
        core = Core(intent.hash, "frozen-exact")
        receipts = []
        for member in intent.old[:intent.quorum]:
            vote = Vote(intent.hash, member, "abort", core.hash)
            slots[member] = slots[member].publish(vote)
            receipts.append(Receipt(vote, vote.hash))
        with self.assertRaises(Rejected):
            certify(intent, core, receipts, slots)
        for member in intent.old[:intent.quorum]:
            slots[member], _ = slots[member].synchronize()
        certify(intent, core, receipts, slots)
        with self.assertRaises(Rejected):
            bad = replace(receipts[0], record_hash="caller-controlled")
            certify(intent, core, [bad, receipts[1]], slots)

    def test_corrupt_foreign_or_rebound_slot_is_not_empty(self):
        intent, slots = fixture()
        member = intent.old[0]
        with self.assertRaises(Rejected):
            replace(slots[member], corrupt=True).publish(
                Vote(intent.hash, member, "abort", "core")
            )
        with self.assertRaises(Rejected):
            slots[member].publish(Vote("other-intent", member, "abort", "core"))
        with self.assertRaises(Rejected):
            slots[member].publish(Vote(intent.hash, replace(member, inode=555), "abort", "core"))

    def test_partial_abort_vetoes_joint_before_quorum(self):
        intent, slots = fixture()
        joint_admitted(intent, slots)
        core = Core(intent.hash, "frozen-exact")
        vote_on(slots, intent.old[0], "abort", core.hash)
        with self.assertRaises(Rejected):
            joint_admitted(intent, slots)

    def test_two_separate_slots_are_an_unsafe_counterexample(self):
        intent, slots = fixture()
        separate_abort_slots = {member: Slot(member, intent.hash) for member in intent.old}
        for member in intent.old[:intent.quorum]:
            vote_on(slots, member, "joint", "proposal")
        core = Core(intent.hash, "frozen-exact")
        receipts = [
            vote_on(separate_abort_slots, member, "abort", core.hash)
            for member in intent.old[-intent.quorum:]
        ]
        certify(intent, core, receipts, separate_abort_slots)
        self.assertGreaterEqual(joint_count(intent, slots), intent.quorum)

    def test_same_members_new_generation_and_publication_gate(self):
        intent, slots = fixture(5)
        for member in intent.old[:2]:
            vote_on(slots, member, "joint", "proposal")
        core = Core(intent.hash, "frozen-exact")
        receipts = [vote_on(slots, member, "abort", core.hash) for member in intent.old[2:]]
        certificate = certify(intent, core, receipts, slots)
        generation = Renewal.from_certificate(intent, certificate)
        self.assertEqual(generation.effective, intent.old)
        self.assertNotEqual(generation.effective, intent.target)
        self.assertTrue(generation.old_fenced)
        self.assertFalse(generation.writable)
        with self.assertRaises(Rejected):
            generation.publish()
        with self.assertRaises(Rejected):
            generation.promote(intent.old[2:], True)
        with self.assertRaises(Rejected):
            generation.promote(intent.old, False)
        promoted = generation.promote(intent.old, True)
        self.assertFalse(promoted.writable)
        published = promoted.publish()
        self.assertTrue(published.writable)
        self.assertTrue(published.old_fenced)
        self.assertEqual(published.candidate_bytes, generation.candidate_bytes)
        self.assertEqual(published.archive_bytes, generation.archive_bytes)
        self.assertEqual(
            published.result[:4],
            (intent.operation, "membership-add-cancelled", intent.hash, intent.fence + 1),
        )
        self.assertEqual(published.result, published.publish().result)


if __name__ == "__main__":
    unittest.main()
