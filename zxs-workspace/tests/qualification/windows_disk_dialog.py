"""Qt selection/decision qualification only. Synthetic original results, no driver, sockets, files or package gate override in production."""
from pathlib import Path
from types import SimpleNamespace as NS
import sys
import unittest
from unittest.mock import patch

if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from PySide6.QtWidgets import QApplication, QWidget
from zipworkspace.desktop import container_windows_controls as ui
from zipworkspace.desktop.container_management import Body, Branch, Container, Node, Outcome, Phase, Receipt, Save, Ticket
from zipworkspace.desktop.container_windows_draft import DraftReview, DraftSource
from zipworkspace.desktop.container_windows_save import WindowsSaveKey
from zipworkspace.desktop.container_windows_submit import WindowsSaveReview


class DesktopDecision(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.app = QApplication.instance() or QApplication([])

    def setUp(self):
        self.c = Container("1"*32, "CON.txt", 2)
        self.n = Node("2"*32, None, 4, "3"*64)
        self.b = Branch("main", self.n.id, (self.n,))
        self.info = NS(container=self.c, branches=(self.b,))
        self.owner = NS(_listed=True, writable=False, operations=object(), _account="fixture",
                        _exchange=NS(_location="https://fixture.invalid"))
        self.panel = QWidget()
        p = self.panel
        p.details, p.listed = {self.c.id: self.info}, {self.c.id: self.c}
        p._epoch, p._owned, p.ops, p._checking_connection, p.jobs = 3, self.owner, self.owner.operations, False, {}
        p._current = lambda: self.c.id
        p.branches, p.nodes = NS(currentData=lambda: self.b), NS(currentData=lambda: self.n)
        p._paint = p._select = lambda: None
        self.ctrl = ui.WindowsDiskControls(NS(pending=None, _container_edit_locked=False), p)
        self.box = None
        self.src = DraftSource("fixture-trust", self.owner._exchange._location, "fixture", self.c, "main", self.n)
        self.key = WindowsSaveKey(self.c.id, "4"*32, "5"*32, "6"*64)
        self.draft_review = DraftReview(self.src, "nonexistent-private-file", 1, b"x"*16, 6, "7"*64)
        body = Body(self.c.id, self.key.submission, "main", self.n.id, 6, self.draft_review.sha256, "main", "8"*32)
        self.ticket = Ticket(Save(self.c, body, "main"), self.key.operation, self.key.submission, 2)
        self.review = WindowsSaveReview(self.key, self.src, self.ticket)

    def tearDown(self):
        if self.box is not None:
            self.box.pool.shutdown(wait=True)
            self.box.timer.stop()
        self.panel.deleteLater()

    def open(self, recovery):
        self.box = ui._Disk(self.ctrl, self.ctrl._scope(recovery), recovery)
        self.box.folder.setText(r"C:\not-accessed")
        return self.box

    def finish(self):
        job = self.box.job
        self.assertIsNotNone(job)
        job.exception(timeout=5)  # Synthetic worker only; GUI production never waits here.
        self.box.poll()

    def test_gate_head_and_local_alias(self):
        with patch.object(ui, "packaged", return_value=False):
            self.assertIsNone(self.ctrl._scope(False))
            self.assertIsNone(self.ctrl._scope(True))
        with patch.object(ui, "packaged", return_value=True):
            self.panel.nodes.currentData = lambda: Node("9"*32, None, 0, "a"*64)
            self.assertIsNone(self.ctrl._scope(False))
            self.assertIsNotNone(self.ctrl._scope(True))
            self.panel.nodes.currentData = lambda: self.n
            box = self.open(False)
            self.assertNotEqual(box.alias.text(), self.c.name)
            self.assertIn(self.c.id[:8], box.alias.text())
            self.assertIsNone(box.job)

    def test_intent_and_retry_are_never_implicit(self):
        effects = []
        receipts = [Phase.UNKNOWN, Phase.CONFIRMED]
        def choice(*_):
            def apply(review, *, confirmed):
                self.assertIs(review, self.review)
                self.assertIs(confirmed, True)
                effects.append("send-original")
                phase = receipts.pop(0)
                r = Receipt(self.ticket, 3, "main", "8"*32, self.n.id, 6, "7"*64) if phase is Phase.CONFIRMED else None
                return Outcome(self.ticket, phase, r)
            return NS(review=self.review, apply=apply)
        with (patch.object(ui, "packaged", return_value=True), patch.object(ui, "admitted_client"),
              patch.object(ui, "WindowsOriginalSaveChoice", side_effect=choice),
              patch.object(ui, "lookup_original_windows_receipt", return_value=Outcome(self.ticket, Phase.ABSENT)) as query,
              patch.object(ui, "confirm", side_effect=[True, False, True]) as consent):
            box = self.open(False)
            box.capture.run = lambda *_: self.draft_review
            def prepare(review, *, confirmed):
                self.assertIs(review, self.draft_review)
                self.assertIs(confirmed, True)
                effects.append("reserve-local")
                return self.key
            box.capture.draft = NS(prepare_save=prepare)
            box.primary(); self.finish()
            self.assertEqual((box.state, effects), ("reviewed", []))
            box.primary(); self.finish()
            self.assertEqual((box.state, effects), ("prepared", ["reserve-local"]))
            box.primary()
            self.assertIsNone(box.job)  # Declined Submit: only the local original exists.
            box.primary(); self.finish()
            self.assertEqual(box.state, "pending")
            box.primary()
            self.assertIsNone(box.job)
            box.query(); self.finish()
            self.assertEqual(box.state, "absent")
            query.assert_called_once_with(self.owner, r"C:\not-accessed", self.key, confirmed=True)
            box.primary(); self.finish()
            self.assertEqual((box.state, effects, consent.call_count), ("complete", ["reserve-local", "send-original", "send-original"], 3))

    def test_seen_server_effect_cannot_be_rearmed_by_later_absence(self):
        receipt = Receipt(self.ticket, 3, "main", "8"*32, self.n.id, 6, "7"*64)
        with (patch.object(ui, "packaged", return_value=True), patch.object(ui, "admitted_client"),
              patch.object(ui, "WindowsOriginalSaveChoice", side_effect=AssertionError("must not prepare a send")),
              patch.object(ui, "lookup_original_windows_receipt", return_value=Outcome(self.ticket, Phase.ABSENT))):
            box = self.open(True)
            box.directory, box.key = box.folder.text(), self.key
            def lost_local_receipt():
                raise ui.WindowsReceiptUncertain(self.key, receipt)
            box.work("send", lost_local_receipt); self.finish()
            self.assertEqual(self.ctrl._known[self.key], receipt)
            box.query(); self.finish()
            self.assertEqual(box.state, "reconcile")
            box.primary()
            self.assertIsNone(box.job)
            self.assertFalse(box.action.isEnabled())

    def test_reserved_key_survives_missing_intent(self):
        with (patch.object(ui, "packaged", return_value=True), patch.object(ui, "admitted_client"),
              patch.object(ui, "WindowsOriginalSaveChoice", side_effect=FileNotFoundError("missing original intent")),
              patch.object(ui, "load_original_windows_key", return_value=self.key)):
            box = self.open(True)
            box.primary(); self.finish()
            self.assertIs(box.key, self.key)
            self.assertIn(ui.display_key(self.key), box.info.toPlainText())
            self.assertEqual(box.state, "pending")
            self.assertIsNone(box.choice)

    def test_cold_recovery_needs_lookup_before_any_send(self):
        with (patch.object(ui, "packaged", return_value=True), patch.object(ui, "admitted_client"),
              patch.object(ui, "WindowsOriginalSaveChoice", return_value=NS(review=self.review)),
              patch.object(ui, "load_original_windows_key", return_value=self.key) as original,
              patch.object(ui, "lookup_original_windows_receipt") as query):
            box = self.open(True)
            box.primary(); self.finish()
            original.assert_called_once_with(self.owner, r"C:\not-accessed", self.c.id, confirmed=True)
            self.assertEqual(box.state, "pending")
            box.primary()
            self.assertIsNone(box.job)
            query.assert_not_called()


if __name__ == "__main__":
    unittest.main()
