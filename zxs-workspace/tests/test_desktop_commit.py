"""Native Linux fixtures for explicit commit from one live desktop checkout, not a registry."""
from dataclasses import replace
import errno
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
from zipworkspace.desktop.branch_choice import SnapshotBranch
from zipworkspace.desktop.checkout_action import CheckoutChoice
from zipworkspace.desktop.commit_choice import CommitChoice
from zipworkspace.desktop.selection import Selection
from zipworkspace.desktop.retry_choice import PendingRetry
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
class OriginalCheckoutFixture(unittest.TestCase):
    needs_freeze = False

    def setUp(self):
        tmp = tempfile.TemporaryDirectory(); self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        private, parent, self.store = (self.root/name for name in ("states", "trees", "store"))
        for path in (private, parent, self.store): path.mkdir(mode=0o700)
        a, b = os.open(private, os.O_DIRECTORY), os.open(parent, os.O_DIRECTORY)
        try:
            try:
                publish.require_native_writer(); private_stage.same_mount(a, b)
                if self.needs_freeze:
                    if not getattr(os, "O_TMPFILE", 0): self.skipTest("anonymous file unsupported")
                    os.close(os.open(".", os.O_TMPFILE | os.O_EXCL | os.O_RDWR, 0o600, dir_fd=a))
            except (OSError, Rejected) as err:
                self.skipTest("actual drive not admitted for this fixture: " + str(err))
        finally: os.close(b); os.close(a)
        self.loose = self.root/"loose.zxs"; self.first = snapshot(self.loose)
        self.original = self.store/self.first.filename
        self.original.write_bytes(self.loose.read_bytes())
        self.tree, self.state = parent/"Active", private/"Active"
        self.live = self.open_live(self.loose, "Active")

    def open_live(self, source, leaf):
        plan = CheckoutChoice(Selection.open(source), keep_session=True); self.addCleanup(plan.close)
        plan.pick_store(self.store); plan.pick_target(self.tree.parent); plan.pick_private(self.state.parent)
        plan.review(leaf); plan.run()
        live = plan.take_session(); self.assertIsNotNone(live)
        self.addCleanup(live.close)
        return live


class ExplicitDesktopCommit(OriginalCheckoutFixture):
    needs_freeze = True

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

    def test_prepared_but_unconfirmed_fresh_choice_cannot_create_a_pending_via_direct_business_entry(self):
        choice = self.choice()
        self.assertTrue(choice.ready); self.assertFalse(choice.used)
        with patch.object(worktree,"anonymous_capture",side_effect=AssertionError("no freeze without confirmation")), \
             patch.object(worktree,"locked",side_effect=AssertionError("no writer before confirmation")), \
             patch.object(publish,"_new_upload",side_effect=AssertionError("no upload before confirmation")), \
             self.assertRaises(Rejected):
            worktree.commit(self.state,selected=choice)
        self.assertFalse((self.state/(choice.operation+".zxs")).exists())
        self.assertIsNone(self.live.status()["pending_operation"])
        self.assertEqual(list(self.store.iterdir()),[self.original])

    def pending_choice(self):
        (self.tree/"story").write_bytes(b"original frozen body")
        original = self.choice()
        with patch.object(publish, "rename_new_at", side_effect=OSError("unconfirmed write")), \
                self.assertRaises(publish.Uncertain): original.run()
        return original

    def retry_choice(self, original):
        self.assertEqual(original.result()[0],"pending")  # Explicit read-only request, not a retry.
        retry = PendingRetry(original); self.addCleanup(retry.release)
        return retry

    def test_resume_requires_a_separate_query_and_replays_only_the_original_still_held_copy(self):
        original = self.pending_choice()
        with self.assertRaises(Rejected): PendingRetry(original)
        retry = self.retry_choice(original)
        with self.assertRaises(Rejected): PendingRetry(original)
        with self.assertRaises(Rejected): retry.run()
        with patch.object(os,"fsync",side_effect=AssertionError("read-only preparation cannot sync")):
            self.assertIn("正式槽不存在",retry.prepare())
        self.assertEqual(retry.operation,original.operation)
        (self.tree/"story").write_bytes(b"edits after freezing are not part of retry")
        self.assertIsNone(retry.ack_fd)
        with patch.object(worktree,"commit",side_effect=AssertionError("not a fresh commit")), \
             patch.object(worktree,"anonymous_capture",side_effect=AssertionError("no spool")), \
             patch.object(worktree,"frozen_tree",side_effect=AssertionError("no worktree scan")), \
             patch.object(publish,"named_document",side_effect=AssertionError("read held candidate only")):
            saved, record = retry.run()
        self.assertEqual(record,original.marker)
        with zipfile.ZipFile(saved) as doc: self.assertEqual(doc.read("story"),b"original frozen body")
        self.assertEqual(original.result()[0],"completed")
        with self.assertRaises(Rejected): retry.run()
        with self.assertRaises(Rejected): PendingRetry(original)

    def test_approval_only_for_existing_result_cannot_fall_back_to_upload_on_loss_or_replacement(self):
        original = self.pending_choice()
        target = self.store/original.marker.filename
        target.write_bytes((self.state/(original.operation+".zxs")).read_bytes())
        for replaced in (False,True):
            with self.subTest(replaced=replaced):
                retry = self.retry_choice(original)
                with patch.object(os,"fsync",side_effect=AssertionError("preview is not a durability barrier")):
                    self.assertIn("只确认这个原文件",retry.prepare())
                self.assertIsNotNone(retry.ack_fd)
                kept = self.root/"confirmed-original.zxs"; target.rename(kept)
                if replaced: target.write_bytes(kept.read_bytes())
                with patch.object(publish,"_new_upload",side_effect=AssertionError("no fallback upload")), \
                        self.assertRaises(Rejected): retry.run()
                self.assertEqual(original.result()[0],"pending")
                if replaced:
                    self.assertNotEqual(kept.stat().st_ino,target.stat().st_ino)
                    target.unlink()
                else: self.assertFalse(target.exists())
                kept.rename(target)
        retry = self.retry_choice(original); retry.prepare()
        with patch.object(publish,"_new_upload",side_effect=AssertionError("only acknowledge existing file")):
            self.assertEqual(retry.run(),(target,original.marker))
        self.assertEqual(original.result()[0],"completed")

    def test_ack_leaf_generation_rechecked_after_real_sync_before_the_database_receipt(self):
        original = self.pending_choice()
        target = self.store/original.marker.filename
        raw = (self.state/(original.operation+".zxs")).read_bytes(); target.write_bytes(raw)
        retry = self.retry_choice(original); retry.prepare()
        actual = retry.store.confirm
        def after_sync(*args, **kwargs):
            result = actual(*args,**kwargs)
            meta = target.stat()
            os.utime(target,ns=(meta.st_atime_ns,meta.st_mtime_ns+1_000_000_000))
            return result
        with patch.object(retry.store,"confirm",side_effect=after_sync), \
             patch.object(publish,"_new_upload",side_effect=AssertionError("never upload")), \
             self.assertRaises(publish.Uncertain): retry.run()
        self.assertEqual(target.read_bytes(),raw)
        self.assertEqual(original.result()[0],"pending")

    def test_equal_byte_different_private_candidate_after_review_does_not_enter_writer(self):
        original = self.pending_choice()
        retry = self.retry_choice(original); retry.prepare()
        candidate = self.state/(original.operation+".zxs")
        kept = self.state/"held-candidate.zxs"; candidate.rename(kept)
        candidate.write_bytes(kept.read_bytes())
        self.assertNotEqual(candidate.stat().st_ino,kept.stat().st_ino)
        with patch.object(worktree,"finish",side_effect=AssertionError("unverified original before writer")), \
                self.assertRaises(Rejected): retry.run()
        self.assertEqual(original.result()[0],"pending")
        self.assertFalse((self.store/original.marker.filename).exists())

    def test_external_cli_completed_during_confirmation_causes_only_original_receipt_lookup(self):
        original = self.pending_choice()
        retry = self.retry_choice(original); retry.prepare()
        target, _ = worktree.finish(self.state, original.operation)
        target.rename(self.root/"no-longer-at-store.zxs")
        with patch.object(worktree,"_publish",side_effect=AssertionError("completed by other client")), \
                self.assertRaisesRegex(Rejected,"已登记完成"): retry.run()
        self.assertEqual(original.result()[0],"completed")
        self.assertFalse(target.exists())

    def test_cli_fork_intent_after_review_is_not_mistaken_for_authority_to_replay_or_follow_it(self):
        original = self.pending_choice()
        retry = self.retry_choice(original); retry.prepare()
        winner = replace(self.first, generation=2, operation="d"*32, parent=self.first.operation)
        target = self.store/winner.filename; snapshot(target,winner); winner_bytes = target.read_bytes()
        with patch.object(worktree,"finish",side_effect=OSError("stop after new intent")), self.assertRaises(OSError):
            worktree.fork_pending(self.state,original.operation)
        with patch.object(worktree,"_publish",side_effect=AssertionError("must not follow successor")), \
                self.assertRaisesRegex(Rejected,"separate explicit fork"): retry.run()
        self.assertEqual(original.result()[0],"forked")
        self.assertEqual(target.read_bytes(),winner_bytes)
        with self.assertRaises(Rejected): PendingRetry(original)

    def test_conflict_seen_during_readonly_review_does_not_offer_replay_or_mutate_the_slot(self):
        original = self.pending_choice()
        retry = self.retry_choice(original)
        winner = replace(self.first, generation=2, operation="d"*32, parent=self.first.operation)
        target = self.store/winner.filename; snapshot(target,winner); before=target.read_bytes()
        with patch.object(os,"fsync",side_effect=AssertionError("no sync on review")), \
             patch.object(publish,"_new_upload",side_effect=AssertionError("no upload")), \
             self.assertRaises(publish.Conflict): retry.prepare()
        with self.assertRaises(Rejected): retry.run()
        self.assertEqual(original.result()[0],"pending")
        self.assertTrue(original.conflict); self.assertFalse(original.pending_seen)
        with self.assertRaises(Rejected): PendingRetry(original)
        self.assertEqual(target.read_bytes(),before)

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

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_window_shows_the_actual_store_capacity_error_and_queries_instead_of_retrying(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import commit_controls
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        def immediate(_title, call, result, *, failed=None):
            try: value = call()
            except Exception as error:
                if failed is None: raise
                failed(error)
            else: result(value)
            return object()
        try:
            controls = win.archive.checkout.commits; controls.set_session(self.live)
            with patch.object(win, "job", side_effect=immediate), patch.object(controls, "refresh_status"), \
                 patch.object(commit_controls, "confirm", return_value=True), \
                 patch.object(publish, "_new_upload", side_effect=OSError(errno.ENOSPC, "injected")) as upload:
                controls.begin()
                detail = controls.output.text()
                self.assertIn("STORE " + str(self.store), detail)
                self.assertIn("ENOSPC", detail); self.assertIn(controls.last.operation, detail)
                self.assertEqual(detail.count("容量诊断："), 1)
                self.assertTrue(controls.query.isEnabled()); controls.inspect()
                self.assertEqual(upload.call_count, 1)
                self.assertEqual(controls.last.result()[0], "pending")
        finally: win.close()

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_window_retry_requires_query_and_its_own_consent_and_cancel_never_publishes(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import commit_controls
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        original = self.pending_choice()
        win = Window()
        def immediate(_title, fn, apply, *, failed=None):
            try: outcome = fn()
            except Exception as error:
                if failed is None: raise
                failed(error)
            else: apply(outcome)
            return object()
        try:
            controls = win.archive.checkout.commits; controls.set_session(self.live)
            controls.last,controls.blocked = original,True; controls.buttons()
            self.assertFalse(controls.retry.isEnabled())
            with patch.object(win,"job",side_effect=immediate), patch.object(controls,"refresh_status"), \
                    patch.object(commit_controls,"confirm",return_value=False) as ask:
                controls.resume(); ask.assert_not_called()
                controls.inspect(); self.assertTrue(controls.retry.isEnabled())
                controls.resume(); ask.assert_called_once()
                self.assertFalse(controls.retry.isEnabled())
                self.assertFalse((self.store/original.marker.filename).exists())
            def approve(_parent,_title,detail,*,attestation,action_text):
                self.assertIn(original.operation,detail)
                self.assertIn("不读取或冻结当前工作文件",detail)
                self.assertIn("STATE",attestation); self.assertIn("STORE",attestation)
                self.assertEqual(action_text,"续作原冻结候选")
                self.assertFalse(controls.retry.isEnabled()); controls.resume()
                return True
            with patch.object(win,"job",side_effect=immediate), patch.object(controls,"refresh_status"), \
                    patch.object(commit_controls,"confirm",side_effect=approve):
                controls.inspect(); controls.resume()
            self.assertTrue((self.store/original.marker.filename).is_file())
            self.assertFalse(controls.retry.isEnabled())
            self.assertEqual(original.result()[0],"completed")
        finally: win.close()


class DisplayedNodeBranch(OriginalCheckoutFixture):
    def proposal(self, live=None):
        job = SnapshotBranch(live or self.live); self.addCleanup(job.release)
        return job

    def test_explicit_fork_uses_the_displayed_parent_not_the_advancing_head_or_dirty_files(self):
        from zipworkspace.refs import Catalog
        (self.tree/"story").write_bytes(b"my uncommitted edits")
        (self.tree/"new").write_bytes(b"not in the displayed node")
        old = self.original.read_bytes(); job = self.proposal()
        with patch.object(os, "O_TMPFILE", 0, create=True), \
             patch.object(worktree, "anonymous_capture", side_effect=AssertionError("no anonymous spool")), \
             patch.object(worktree, "frozen_tree", side_effect=AssertionError("do not read or freeze the working files")):
            with patch.object(os, "fsync", side_effect=AssertionError("confirmation must be read-only")):
                detail = job.prepare()
            for expected in (self.first.operation, self.first.branch, job.operation, job.branch_id,
                             job.proposed.filename, self.state, self.store):
                self.assertIn(str(expected), detail)
            self.assertEqual(list(self.store.iterdir()), [self.original])
            self.assertIsNone(self.live.status()["pending_operation"])
            later = replace(self.first, generation=2, operation="e"*32, parent=self.first.operation)
            advanced = self.store/later.filename; snapshot(advanced, later)
            path, node = job.run()
        self.assertEqual((node.repo,node.branch,node.generation,node.operation,node.parent,node.tree),
                         (self.first.repo,job.branch_id,1,job.operation,self.first.operation,self.first.tree))
        self.assertNotEqual(node.branch, self.first.branch)
        self.assertEqual(path, self.store/node.filename); self.assertEqual(self.original.read_bytes(), old)
        self.assertEqual(path.read_bytes(), (self.state/(job.operation+".zxs")).read_bytes())
        self.assertNotEqual(path.read_bytes(), old)
        with zipfile.ZipFile(path) as z:
            self.assertEqual(z.read("story"), b"original story"); self.assertIn("empty/", z.namelist())
            self.assertNotIn("new", z.namelist())
        self.assertEqual((self.tree/"story").read_bytes(), b"my uncommitted edits")
        self.assertEqual((self.tree/"new").read_bytes(), b"not in the displayed node")
        catalog = Catalog(self.original, self.store)
        self.assertEqual(catalog.select_branch(self.first.branch).marker, later)
        self.assertEqual(catalog.select_branch(node.branch).marker, node)
        self.assertEqual(self.live.status()["branch"], node.branch)
        self.assertEqual(job.result()[0], "completed")
        with worktree.locked(self.state, read_only=True) as (con, _, __):
            self.assertEqual(con.execute("SELECT COUNT(*) FROM forked_original").fetchone()[0], 0)

    def test_a_byte_identical_replacement_of_the_displayed_loose_archive_is_not_the_selected_source(self):
        job = self.proposal(); job.prepare()
        preserved = self.root/"previous-source.zxs"
        self.loose.rename(preserved); self.loose.write_bytes(preserved.read_bytes())
        with patch.object(worktree, "locked", side_effect=AssertionError("no SQL writer")), self.assertRaises(Rejected):
            job.run()
        self.assertEqual(list(self.store.iterdir()), [self.original]); self.assertFalse((self.state/(job.operation+".zxs")).exists())

    def test_displayed_canonical_slot_reuses_its_verified_handle_and_only_its_original_generation(self):
        canonical = self.open_live(self.original, "Canonical")
        job = self.proposal(canonical)
        with patch("zipworkspace.desktop.branch_choice.named_document", side_effect=AssertionError("same slot already held")):
            job.prepare()
        self.assertIs(job.original, job.source)
        old = self.root/"old-canonical.zxs"; self.original.rename(old); self.original.write_bytes(old.read_bytes())
        with self.assertRaises(Rejected): job.run()
        self.assertIsNone(canonical.status()["pending_operation"])

    def test_same_tree_with_different_canonical_zip_bytes_never_supplies_the_parent(self):
        self.original.rename(self.root/"first-canonical")
        snapshot(self.original, self.first, year=1995)
        self.assertNotEqual(self.original.read_bytes(), self.loose.read_bytes())
        job = self.proposal()
        with self.assertRaisesRegex(Rejected, "SHA-256"): job.prepare()
        with patch.object(worktree, "locked", side_effect=AssertionError("no new operation after refusal")), self.assertRaises(Rejected):
            job.run()
        self.assertFalse((self.state/(job.operation+".zxs")).exists())

    def test_unconfirmed_cancelled_and_wrong_purpose_decisions_cannot_cross_into_another_writer(self):
        job = self.proposal(); job.prepare()
        with patch.object(worktree, "anonymous_capture", side_effect=AssertionError("must not freeze")), \
             patch.object(worktree, "locked", side_effect=AssertionError("must not enter writer")):
            with self.assertRaises(Rejected): worktree.branch_from_snapshot(self.state, selected=job)
            with self.assertRaises(Rejected): PendingRetry(job)
            with patch.object(worktree, "branch_from_snapshot", side_effect=lambda path, *, selected:
                              worktree.commit(path, selected=selected)), self.assertRaises(Rejected):
                job.run()  # used=True, but consent to branch is not consent to freeze.
        cancelled = self.proposal(); cancelled.prepare(); cancelled.close()
        with self.assertRaises(Rejected): cancelled.run()
        self.assertIsNone(self.live.status()["pending_operation"]); self.assertEqual(list(self.store.iterdir()), [self.original])

    def test_raced_sqlite_baseline_refuses_without_reading_a_new_head_or_writing_another_candidate(self):
        first, other = self.proposal(), self.proposal()
        first.prepare(); other.prepare(); _, winner = other.run()
        with self.assertRaisesRegex(Rejected, "基线已改变"): first.run()
        next_job = self.proposal()
        with patch.object(next_job,"_open_base",side_effect=AssertionError("do not inspect another node")), self.assertRaises(Rejected):
            next_job.prepare()
        self.assertFalse((self.state/(first.operation+".zxs")).exists())
        self.assertEqual(self.live.status()["base_operation"], winner.operation)

    def test_other_original_pending_wins_without_consuming_it_or_making_a_second_candidate(self):
        first, other = self.proposal(), self.proposal()
        first.prepare(); other.prepare()
        with patch.object(worktree, "finish", side_effect=OSError(errno.EIO, "after original intent")), self.assertRaises(OSError):
            other.run()
        with patch.object(first.area, "copy_with_marker", side_effect=AssertionError("do not capture competing operation")), \
                self.assertRaisesRegex(Rejected, "已有待决"):
            first.run()
        self.assertEqual(self.live.status()["pending_operation"], other.operation)
        self.assertEqual(first.result()[0], "other")
        self.assertTrue((self.state/(other.operation+".zxs")).is_file())
        self.assertFalse((self.state/(first.operation+".zxs")).exists())
        self.assertEqual(list(self.store.iterdir()), [self.original])

    def test_new_first_generation_occupied_before_or_during_promotion_is_never_replaced_or_renumbered(self):
        early = self.proposal(); early.prepare()
        early_slot = self.store/early.proposed.filename; early_slot.write_bytes(b"belongs to somebody else")
        with self.assertRaises(publish.Conflict): early.run()
        self.assertEqual(early_slot.read_bytes(), b"belongs to somebody else")
        self.assertFalse((self.state/(early.operation+".zxs")).exists())
        later = self.proposal(); later.prepare()
        target, rename = self.store/later.proposed.filename, publish.rename_new_at
        def win_at_rename(fd, stage, name):
            self.assertEqual(name, target.name)
            target.write_bytes(b"race winner")
            rename(fd, stage, name)
        with patch.object(publish, "rename_new_at", side_effect=win_at_rename), self.assertRaises(publish.Conflict):
            later.run()
        self.assertEqual(target.read_bytes(), b"race winner")
        self.assertEqual((later.result()[0], later.pending_seen), ("pending", False))
        self.assertEqual(self.live.status()["branch"], self.first.branch)
        self.assertEqual(self.live.status()["pending_operation"], later.operation)
        self.assertTrue((self.state/(later.operation+".zxs")).exists())
        with self.assertRaises(Rejected): PendingRetry(later)

    def test_exact_published_branch_slot_needs_new_consent_to_switch_without_a_second_upload(self):
        job = self.proposal(); job.prepare()
        actual_publish = worktree._publish
        def visible_before_lost_receipt(*args, **kwargs):
            target = actual_publish(*args, **kwargs)
            self.assertEqual(target, self.store/job.proposed.filename)
            raise OSError(errno.EIO, "formal slot durable; local result not recorded")
        with patch.object(worktree, "_publish", side_effect=visible_before_lost_receipt), self.assertRaises(OSError):
            job.run()
        before = self.live.status()
        self.assertEqual((before["branch"], before["pending_operation"]), (self.first.branch, job.operation))
        target = self.store/job.proposed.filename; original = target.stat()
        self.assertEqual(target.read_bytes(), (self.state/(job.operation+".zxs")).read_bytes())
        (self.tree/"story").write_bytes(b"my edits after publication")
        self.assertEqual(job.result()[0], "pending")
        retry = PendingRetry(job); self.addCleanup(retry.release)
        with patch.object(os,"fsync",side_effect=AssertionError("preview is not acknowledgement")):
            detail = retry.prepare()
        self.assertIn(f"当前分支：{self.first.branch}", detail)
        self.assertIn(f"目标分支：{job.branch_id} · 第 1 代", detail)
        self.assertIn("不上传", detail); self.assertIn("工作目录文件不改", detail)
        self.assertEqual(retry.confirmation_label, "确认原槽并切换工作树分支")
        self.assertIsNotNone(retry.ack_fd)
        self.assertEqual(self.live.status()["branch"], self.first.branch)
        with patch.object(publish,"_new_upload",side_effect=AssertionError("ack cannot upload")), \
             patch.object(publish,"rename_new_at",side_effect=AssertionError("ack cannot rename")), \
             patch.object(worktree,"frozen_tree",side_effect=AssertionError("ack cannot inspect working files")):
            saved, marker = retry.run()
        self.assertEqual((saved, marker), (target, job.proposed)); self.assertTrue(os.path.samestat(original, target.stat()))
        after = self.live.status()
        self.assertEqual((after["branch"], after["pending_operation"]), (job.branch_id, None))
        self.assertEqual((self.tree/"story").read_bytes(), b"my edits after publication")

    def test_unknown_then_explicit_original_receipt_replay_never_freezes_or_chooses_a_new_branch(self):
        job = self.proposal(); job.prepare()
        with patch.object(publish, "rename_new_at", side_effect=OSError(errno.EIO, "no original answer")), \
                self.assertRaises(publish.Uncertain): job.run()
        self.assertFalse((self.store/job.proposed.filename).exists())
        self.assertEqual(self.live.status()["branch"], self.first.branch)
        self.loose.rename(self.root/"not-needed-for-recorded-replay.zxs")
        (self.tree/"story").write_bytes(b"changed again")
        with patch.object(worktree,"finish", side_effect=AssertionError("query is not publication")):
            self.assertEqual(job.result()[0], "pending")
        retry = PendingRetry(job); self.addCleanup(retry.release); detail = retry.prepare()
        self.assertIn(f"当前分支：{self.first.branch}", detail); self.assertIn(f"目标分支：{job.branch_id}", detail)
        self.assertEqual(retry.confirmation_label, "发布原候选并切换工作树分支"); self.assertIsNone(retry.ack_fd)
        with patch.object(worktree, "anonymous_capture", side_effect=AssertionError("never freeze on replay")), \
             patch.object(worktree, "branch_from_snapshot", side_effect=AssertionError("no second branch proposal")):
            path, marker = retry.run()
        self.assertEqual(marker, job.proposed); self.assertEqual(path, self.store/job.proposed.filename)
        self.assertEqual(self.live.status()["branch"], job.branch_id)
        self.assertEqual((self.tree/"story").read_bytes(), b"changed again")

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_branch_button_requires_its_own_approval_and_a_still_selected_original_session(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import commit_controls
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        def immediate(_title, call, accept, *, failed=None):
            try: value = call()
            except Exception as err:
                if failed is None: raise
                failed(err)
            else: accept(value)
            return object()
        try:
            controls = win.archive.checkout.commits; controls.set_session(self.live)
            def switch(_parent, title, text, *, attestation, action_text):
                self.assertEqual(action_text, "创建、发布并切换")
                self.assertIn(self.first.operation, text); self.assertIn("不含当前编辑", attestation)
                controls.set_session(None)
                return True  # Consent for a vanished window selection is not a new authorization.
            with patch.object(win, "job", side_effect=immediate), patch.object(controls, "refresh_status"), \
                 patch.object(commit_controls, "confirm", side_effect=switch), \
                 patch.object(worktree, "branch_from_snapshot", side_effect=AssertionError("stale selection")):
                controls.branch()
            self.assertFalse(controls.new_branch.isEnabled())
            controls.set_session(self.live)
            with patch.object(win,"job", side_effect=immediate), patch.object(controls,"refresh_status"), \
                 patch.object(commit_controls,"confirm",return_value=False), \
                 patch.object(worktree,"branch_from_snapshot",side_effect=AssertionError("not confirmed")):
                controls.branch()
            self.assertEqual(list(self.store.iterdir()), [self.original]); self.assertIsNone(controls.last)
            def approve(_parent, title, text, *, attestation, action_text):
                self.assertIn("从所选原节点", title); self.assertIn(str(self.store), text)
                self.assertEqual(action_text, "创建、发布并切换"); self.assertFalse(controls.new_branch.isEnabled())
                controls.branch()  # No second job or ID while the first decision is active.
                return True
            with patch.object(win,"job", side_effect=immediate), patch.object(controls,"refresh_status"), \
                 patch.object(commit_controls,"confirm", side_effect=approve) as ask:
                controls.branch()
            self.assertEqual(ask.call_count, 1); self.assertIn("新分支已发布", controls.output.text())
            self.assertIn(controls.last.branch_id, controls.output.text())
            self.assertEqual(self.live.status()["pending_operation"], None)
            self.assertEqual(len(list(self.store.iterdir())), 2)
        finally: win.close()
