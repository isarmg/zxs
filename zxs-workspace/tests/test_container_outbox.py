"""Source fixtures: durable local R3 originals, no credentials or network."""
from contextlib import contextmanager
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest

from zipworkspace.container_http import ConfirmedAction
from zipworkspace.container_plan import ReceiptFence
from zipworkspace.desktop.container_client import Original
from zipworkspace.desktop.container_management import Container, Phase, Recycle, Rename, Tags
from zipworkspace.desktop.container_outbox import NativeOutbox
from zipworkspace.model import Rejected

DESTINATION = "d" * 64
A = Container("1" * 32, "one.zxs", 4)
B = Container("2" * 32, "two.zxs", 8)


@unittest.skipUnless(sys.platform.startswith("linux"), "local R3 native custody is Linux-only")
class OriginalOutbox(unittest.TestCase):
    @contextmanager
    def state(self):
        with tempfile.TemporaryDirectory() as name:
            path = Path(name)
            yield path, NativeOutbox.create(path, DESTINATION, account="fixture-owner")

    def test_exact_original_survives_reopen_and_is_never_silently_reallocated(self):
        with self.state() as (path, first):
            change = Rename(A, "after.zxs")
            with first.hold(A.id):
                original = first.reserve(change)
                self.assertEqual(first.prefix(original.submission, A.id), ())
            recovered = NativeOutbox(path, DESTINATION, account="fixture-owner")
            pending = recovered.pending(A.id)
            self.assertEqual(pending.phase, Phase.PENDING)
            self.assertEqual(pending.ticket.operation, original.operation)
            with recovered.hold(A.id):
                self.assertEqual(recovered.reserve(change), original)
                with self.assertRaises(Rejected):
                    recovered.reserve(Rename(A, "different.zxs"))
                confirmed = ConfirmedAction(ReceiptFence(A.id, original.operation, "rename", 4, 5), None)
                recovered.record(original, confirmed)
                recovered.record(original, confirmed)  # same result, no second receipt
                recovered.require_prepared(original, change, settled=True)
                with self.assertRaises(Rejected):
                    recovered.require_prepared(original, change)
                self.assertEqual(recovered.prefix(original.submission, A.id), (confirmed.fence,))
            self.assertIsNone(NativeOutbox(path, DESTINATION, account="fixture-owner").pending(A.id))

    def test_transport_admission_requires_exact_durable_ticket_under_lease(self):
        with self.state() as (path, journal):
            change = Rename(A, "after.zxs")
            with journal.hold(A.id):
                original = journal.reserve(change)
                journal.require_prepared(original, change)
                with self.assertRaises(Rejected):
                    journal.require_prepared(original, change, settled=True)
                guesses = [
                    (Original(original.submission, "e" * 32), change),
                    (original, Rename(A, "different.zxs")),
                ]
                for guessed, proposed in guesses:
                    with self.assertRaises(Rejected):
                        journal.require_prepared(guessed, proposed)
            with self.assertRaises(Rejected):
                journal.require_prepared(original, change)
            registry = next(path.glob("outbox-*.sqlite"))
            with sqlite3.connect(registry) as fd:
                fd.execute("UPDATE tickets SET body=?", (b"corrupt",))
            with journal.hold(A.id), self.assertRaises(Rejected):
                journal.require_prepared(original, change)
            with sqlite3.connect(registry) as fd:
                fd.execute("DELETE FROM tickets")
            with journal.hold(A.id), self.assertRaises(Rejected):
                journal.require_prepared(original, change)
            with sqlite3.connect(registry) as fd:
                self.assertEqual(fd.execute("SELECT count(*) FROM tickets").fetchone(), (0,))

    def test_leases_and_receipts_are_per_container(self):
        with self.state() as (path, first):
            second = NativeOutbox(path, DESTINATION, account="fixture-owner")
            with first.hold(A.id):
                old = first.reserve(Recycle(A))
                with second.hold(B.id):
                    tags = second.reserve(Tags(B, ("留存",)))
                    second.record(tags, ConfirmedAction(
                        ReceiptFence(B.id, tags.operation, "set_tags", 8, 9), None))
                with self.assertRaises(Rejected), second.hold(A.id):
                    self.fail("same container acquired two leases")
                with self.assertRaises(Rejected):
                    first.record(old, ConfirmedAction(
                        ReceiptFence(A.id, old.operation, "delete", 3, None), True))
                self.assertEqual(first.prefix(old.submission, A.id), ())
                first.record(old, ConfirmedAction(
                    ReceiptFence(A.id, old.operation, "delete", 4, None), True))
            self.assertIsNone(second.pending(A.id))
            self.assertIsNone(second.pending(B.id))

    def test_missing_live_recycle_recovers_only_the_same_existing_original(self):
        with self.state() as (path, journal):
            with journal.hold(A.id):
                original = journal.reserve(Recycle(A))
            with journal.hold(B.id):
                journal.reserve(Rename(B, "renamed.zxs"))
            reopened = NativeOutbox(path, DESTINATION, account="fixture-owner")
            self.assertEqual(reopened.pending_recycles(frozenset({A.id})), ((), True))
            rows, complete = reopened.pending_recycles(frozenset())
            self.assertTrue(complete)
            self.assertEqual(len(rows), 1)
            self.assertIs(rows[0].phase, Phase.PENDING)
            self.assertEqual(rows[0].ticket.change, Recycle(A))
            self.assertEqual(rows[0].ticket.operation, original.operation)
            self.assertEqual(rows[0].ticket.submission, original.submission)
            self.assertIsNone(rows[0].receipt)
            lock = next(path.glob(f"*{A.id}.lock"))
            lock.unlink()
            self.assertEqual(reopened.pending_recycles(frozenset()), ((), False))
            self.assertFalse(lock.exists())

    def test_missing_or_corrupt_selected_outbox_never_bootstraps_a_new_original(self):
        with self.state() as (path, journal):
            with journal.hold(A.id):
                original = journal.reserve(Rename(A, "after.zxs"))
            with self.assertRaises(Rejected):
                NativeOutbox(path, "a" * 64, account="fixture-owner")
            with self.assertRaises(Rejected):
                NativeOutbox(path, DESTINATION, account="different-owner")
            registry = next(path.glob("outbox-*.sqlite"))
            with sqlite3.connect(registry) as fd:
                fd.execute("UPDATE plans SET digest=?", ("0" * 64,))
            with self.assertRaises(Rejected), journal.hold(A.id):
                journal.reserve(Rename(A, "after.zxs"))
            registry.rename(path / "unknown-local-record")
            with self.assertRaises(Rejected):
                journal.require_available()
            with self.assertRaises(Rejected), journal.hold(B.id):
                self.fail("hold qualified a missing registry")
            self.assertFalse(any(path.glob(f"*{B.id}.lock")))
            with self.assertRaises(Rejected):
                NativeOutbox(path, DESTINATION, account="fixture-owner")
            with self.assertRaises(Rejected):
                NativeOutbox.create(path, DESTINATION, account="fixture-owner")
            self.assertTrue((path / (original.submission + ".pending")).is_dir())
