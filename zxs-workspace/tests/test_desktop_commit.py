"""Native Linux fixtures for explicit commit from one live desktop checkout, not a registry."""
from dataclasses import replace
import hashlib
from importlib.util import find_spec
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import private_stage, publish, worktree
from zipworkspace.desktop.checkout_action import CheckoutChoice
from zipworkspace.desktop.commit_choice import CommitChoice
from zipworkspace.desktop.selection import Selection
from zipworkspace.model import Commit, Manifest, Rejected


def snapshot(path, marker=None, *, year=1980):
    data = b"original story"
    rows = Manifest()
    rows.add("story", False, len(data), hashlib.sha256(data).hexdigest())
    rows.add("empty", True, 0, None)
    marker = marker or Commit("a"*32, "b"*32, 1, "c"*32, None, rows.digest())
    with zipfile.ZipFile(path, "x", compression=zipfile.ZIP_STORED) as out:
        for name, body, mode in (("story", data, 0o100644), ("empty/", b"", 0o40755)):
            entry = zipfile.ZipInfo(name, (year, 1, 1, 0, 0, 0)); entry.external_attr = mode << 16
            out.writestr(entry, body)
        out.comment = marker.encode()
    return marker


@unittest.skipUnless(sys.platform.startswith("linux"), "real admitted Linux ext4/XFS required")
class ExplicitDesktopCommit(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory(); self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        private, parent, self.store = (self.root/name for name in ("states", "trees", "store"))
        for path in (private, parent, self.store): path.mkdir(mode=0o700)
        a, b = os.open(private, os.O_DIRECTORY), os.open(parent, os.O_DIRECTORY)
        try:
            try:
                publish.require_native_writer(); private_stage.same_mount(a, b)
                if not getattr(os, "O_TMPFILE", 0): self.skipTest("anonymous file unsupported")
                os.close(os.open(".", os.O_TMPFILE | os.O_EXCL | os.O_RDWR, 0o600, dir_fd=a))
            except (OSError, Rejected) as err:
                self.skipTest("actual drive not admitted for this fixture: " + str(err))
        finally: os.close(b); os.close(a)
        self.loose = self.root/"loose.zxs"; self.first = snapshot(self.loose)
        self.original = self.store/self.first.filename
        self.original.write_bytes(self.loose.read_bytes())
        plan = CheckoutChoice(Selection.open(self.loose), keep_session=True)
        self.addCleanup(plan.close)
        plan.pick_store(self.store); plan.pick_target(parent); plan.pick_private(private)
        plan.review("Active"); plan.run()
        self.live = plan.take_session(); self.assertIsNotNone(self.live)
        self.addCleanup(self.live.close)
        self.tree, self.state = parent/"Active", private/"Active"

    def choice(self, *, review=True):
        choice = CommitChoice(self.live); self.addCleanup(choice.release)
        if review:
            text = choice.prepare()
            for value in (self.state, self.tree, self.store, choice.operation, self.first.operation):
                self.assertIn(str(value), text)
        return choice

    def test_commit_publishes_exact_new_branch_slot_and_only_observes_the_final_receipt(self):
        (self.tree/"story").write_bytes(b"saved by the user")
        before = self.original.read_bytes()
        choice = self.choice()
        self.assertIsNone(self.live.status()["pending_operation"])
        self.assertEqual(list(self.store.iterdir()), [self.original])
        with patch.object(worktree, "publish", side_effect=AssertionError("must borrow the held STORE")):
            path, marker = choice.run()
        self.assertEqual((marker.generation, marker.branch, marker.parent, marker.operation),
                         (2, self.first.branch, self.first.operation, choice.operation))
        self.assertEqual(path, self.store/marker.filename)
        self.assertEqual(self.original.read_bytes(), before)
        with zipfile.ZipFile(path) as doc:
            self.assertEqual(doc.read("story"), b"saved by the user")
            self.assertIn("empty/", doc.namelist())
        self.assertEqual(self.live.status()["base_operation"], choice.operation)
        path.rename(self.root/"moved-after-success.zxs")
        with patch.object(worktree,"finish",side_effect=AssertionError("query must not publish")):
            state, message = choice.result()
        self.assertEqual(state, "completed"); self.assertIn("历史回执", message)
        self.assertFalse(path.exists())
        with self.assertRaises(Rejected): choice.run()

    def test_missing_canonical_base_does_not_publish_the_visible_loose_document(self):
        self.original.unlink()
        with patch.object(type(self.live),"check_catalog",side_effect=AssertionError("no path I/O at button press")):
            choice = self.choice(review=False)
        with self.assertRaisesRegex(Rejected, "STORE 中缺少原基线"): choice.prepare()
        self.assertTrue(self.loose.is_file()); self.assertFalse(any(self.store.iterdir()))
        self.assertEqual(choice.result()[0], "unrecorded")

    def test_failed_complete_baseline_verification_permanently_refuses_both_writer_entries(self):
        retained = self.root/"original-baseline.zxs"; self.original.rename(retained)
        for bad_marker, year in ((replace(self.first,operation="e"*32),1980), (self.first,2007)):
            with self.subTest(operation=bad_marker.operation, year=year):
                snapshot(self.original, bad_marker, year=year)
                before = self.original.read_bytes()
                choice = self.choice(review=False)
                with self.assertRaisesRegex(Rejected, "整包 SHA-256 不符"): choice.prepare()
                with patch.object(worktree,"anonymous_capture",side_effect=AssertionError("no capture")), \
                     patch.object(worktree,"locked",side_effect=AssertionError("no SQLite open")), \
                     patch.object(publish,"_new_upload",side_effect=AssertionError("no STORE output")):
                    with self.assertRaises(Rejected): choice.run()
                    with self.assertRaises(Rejected): worktree.commit(self.state,selected=choice)
                    with self.assertRaises(Rejected): choice.prepare()
                self.assertEqual(self.original.read_bytes(), before)
                self.assertFalse((self.state/(choice.operation+".zxs")).exists())
                self.assertIsNone(self.live.status()["pending_operation"])
                self.original.unlink()
        retained.rename(self.original)

    def test_intervening_real_cli_commit_invalidates_decision_without_a_new_candidate(self):
        choice = self.choice()
        (self.tree/"story").write_bytes(b"concurrent agreed CLI commit")
        _, other = worktree.commit(self.state)
        with patch.object(worktree,"frozen_tree",side_effect=AssertionError("changed baseline must not capture")), \
                self.assertRaisesRegex(Rejected, "基线已改变"): choice.run()
        self.assertFalse((self.state/(choice.operation+".zxs")).exists())
        self.assertEqual(self.live.status()["base_operation"], other.operation)
        self.assertCountEqual(self.store.iterdir(), (self.original,self.store/other.filename))
        self.assertEqual(choice.result()[0], "unrecorded")

    def test_equal_byte_replacement_of_reviewed_slot_is_not_the_confirmed_original(self):
        choice = self.choice()
        original = self.root/"kept-original.zxs"; self.original.rename(original)
        self.original.write_bytes(original.read_bytes())
        self.assertNotEqual(self.original.stat().st_ino, original.stat().st_ino)
        with patch.object(worktree,"anonymous_capture",side_effect=AssertionError("must check before capture")), \
                self.assertRaises(Rejected): choice.run()
        self.assertFalse((self.state/(choice.operation+".zxs")).exists())
        self.assertEqual(self.original.read_bytes(), self.loose.read_bytes())
        self.assertEqual(choice.result()[0], "unrecorded")

    def test_unknown_rename_keeps_one_pending_id_and_query_never_resumes(self):
        (self.tree/"story").write_bytes(b"only frozen copy")
        choice = self.choice()
        with patch.object(publish, "rename_new_at", side_effect=OSError("lost reply")), \
                self.assertRaises(publish.Uncertain): choice.run()
        before = {p.name for p in self.store.iterdir()}
        self.assertTrue(any(n.endswith(".partial") for n in before))
        with patch.object(worktree,"finish",side_effect=AssertionError("not a retry")), \
             patch.object(worktree,"_publish",side_effect=AssertionError("not a writer")):
            self.assertEqual(choice.result()[0],"pending")
        self.assertEqual(before,{p.name for p in self.store.iterdir()})
        self.assertEqual(self.live.status()["pending_operation"], choice.operation)
        (self.tree/"story").write_bytes(b"edited after interruption")
        path, marker = worktree.finish(self.state,choice.operation)  # Deliberate CLI retry, original op only.
        self.assertEqual(marker.operation, choice.operation)
        with zipfile.ZipFile(path) as doc: self.assertEqual(doc.read("story"), b"only frozen copy")
        self.assertEqual(choice.result()[0], "completed")

    def test_genuine_occupied_generation_is_never_overwritten_or_automatically_forked(self):
        choice = self.choice()
        winner = replace(self.first, generation=2, operation="d"*32, parent=self.first.operation)
        slot = self.store/winner.filename; snapshot(slot, winner); winner_bytes = slot.read_bytes()
        with self.assertRaises(publish.Conflict): choice.run()
        self.assertEqual(slot.read_bytes(), winner_bytes)
        self.assertEqual(choice.result()[0], "pending")
        self.assertCountEqual(self.store.iterdir(), (self.original,slot))
        self.assertTrue((self.state/(choice.operation+".zxs")).is_file())
        with patch.object(worktree,"finish",side_effect=OSError("stop after explicit fork intent")), \
                self.assertRaises(OSError):
            worktree.fork_pending(self.state, choice.operation)
        kind, text = choice.result()
        self.assertEqual(kind,"forked")
        self.assertIn(self.live.status()["pending_operation"],text)
        self.assertIn("不表示新分支已发布",text)
        self.assertCountEqual(self.store.iterdir(), (self.original,slot))

    def test_equal_named_substitute_store_cannot_receive_even_the_upload_stage(self):
        choice = self.choice()
        old = self.root/"selected-original-store"
        create = publish._new_upload
        def replaced(scope):
            self.store.rename(old); self.store.mkdir(mode=0o700)
            return create(scope)
        with patch.object(publish, "_new_upload", side_effect=replaced), self.assertRaises(Rejected):
            choice.run()
        self.assertFalse(any(self.store.iterdir()))
        self.assertEqual(list(old.iterdir()), [old/self.first.filename])
        self.store.rmdir(); old.rename(self.store)
        self.assertEqual(choice.result()[0], "pending")

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_window_cancels_without_writing_then_commits_once_after_fresh_consent(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import commit_controls
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        try:
            controls = win.archive.checkout.commits; controls.set_session(self.live)
            def immediate(_title, fn, apply, *, failed=None):
                try: result = fn()
                except Exception as e:
                    if failed is None: raise
                    failed(e)
                else: apply(result)
                return object()
            def approve(_parent, _title, detail, *, attestation, action_text):
                self.assertIn(str(self.tree), detail); self.assertIn(str(self.state), detail)
                self.assertIn(self.first.operation, detail); self.assertIn("写入者",attestation)
                self.assertIn("STORE 不同步",attestation); self.assertEqual(action_text,"确认并提交")
                self.assertFalse(controls.save.isEnabled()); controls.begin()
                return True
            with patch.object(win,"job",side_effect=immediate), \
                 patch.object(controls,"refresh_status"), \
                 patch.object(commit_controls,"confirm",return_value=False):
                controls.begin()
            self.assertEqual(self.live.status()["base_operation"],self.first.operation)
            self.assertIsNone(controls.last)
            with patch.object(win,"job",side_effect=immediate), \
                 patch.object(controls,"refresh_status"), \
                 patch.object(commit_controls,"confirm",side_effect=approve):
                controls.begin()
                self.assertTrue(controls.query.isEnabled())
                controls.inspect()
                self.assertIn("历史回执",controls.output.text())
            self.assertEqual(len(tuple(self.store.iterdir())),2)
        finally: win.close()
