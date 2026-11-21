"""Native Linux fixtures for explicit commit from one live desktop checkout, not a registry."""
from concurrent.futures import Future
from contextlib import contextmanager
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

from zipworkspace import lifecycle, private_stage, publish, worktree
from zipworkspace.desktop.branch_choice import SnapshotBranch
from zipworkspace.desktop.checkout_action import CheckoutChoice
from zipworkspace.desktop.commit_choice import CommitChoice, CommitStopped
from zipworkspace.desktop.pending_fork import PendingFork
from zipworkspace.desktop.selection import Selection, Verification, read_registered_baseline
from zipworkspace.desktop.retry_choice import PendingRetry
from zipworkspace.model import CHUNK, Commit, Manifest, Rejected


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
        archive = Selection.open(source, retain=True)
        self.addCleanup(archive.release_display)
        plan = CheckoutChoice(archive, keep_session=True); self.addCleanup(plan.close)
        plan.pick_store(self.store); plan.pick_target(self.tree.parent); plan.pick_private(self.state.parent)
        plan.review(leaf); plan.run()
        live = plan.take_session(); self.assertIsNotNone(live)
        self.addCleanup(live.close)
        return live


class LiveChangesEvidence(OriginalCheckoutFixture):
    def baseline_display(self):
        m = self.first
        return m.repo, m.branch, m.generation, m.operation

    def test_baseline_browser_reads_the_canonical_slot_not_work_or_a_new_head(self):
        from zipworkspace.desktop import selection
        real_connect = worktree.sqlite3.connect
        modes = []
        def only_ro(name, *args, **kw):
            self.assertTrue(kw.get("uri"))
            self.assertTrue(name.endswith("?mode=ro"))
            modes.append(name)
            return real_connect(name, *args, **kw)
        (self.tree/"story").write_bytes(b"not in this archived base")
        tip = Commit(self.first.repo, self.first.branch, 2, "d"*32, self.first.operation, self.first.tree)
        snapshot(self.store / tip.filename, tip)
        with patch.object(worktree.sqlite3, "connect", side_effect=only_ro), \
             patch.object(selection, "named_document", side_effect=AssertionError("no reopen by spelling")), \
             patch.object(selection, "Catalog", side_effect=AssertionError("no store inventory / latest head")), \
             patch.object(lifecycle, "observe_held", side_effect=AssertionError("no work content read")), \
             patch.object(os, "fsync", side_effect=AssertionError("no writing")):
            view = read_registered_baseline(self.live, self.baseline_display(), observer=Verification())
        self.addCleanup(view.release_display)
        self.assertEqual(len(modes), 2)  # short committed observations on either side of the archive read
        self.assertEqual((view.path, view.browser.commit), (self.original, self.first))
        self.assertEqual(view.exact.sha256, hashlib.sha256(self.original.read_bytes()).hexdigest())
        self.assertTrue(os.path.samestat(os.fstat(view.held.fileno()), self.original.stat()))
        self.assertEqual(view.browser.list_directory().entries[1].size_bytes, len(b"original story"))

    def test_baseline_browser_rejects_slot_replacement_at_the_final_gate(self):
        from zipworkspace.desktop import selection
        raw = self.original.read_bytes()
        retained, real = self.root/"held-original.zxs", selection.archive_sha
        def replace_after_hash(*args, **kw):
            digest = real(*args, **kw)
            self.original.rename(retained)
            self.original.write_bytes(raw)  # the selected post-hash name is not this byte-identical inode
            return digest
        with patch.object(selection, "archive_sha", side_effect=replace_after_hash):
            with self.assertRaisesRegex(Rejected, "原正式名称或挂载"):
                read_registered_baseline(self.live, self.baseline_display(), observer=Verification())
        self.assertEqual(retained.read_bytes(), self.original.read_bytes())

    def test_baseline_browser_does_not_follow_a_concurrent_change_in_the_original_registration(self):
        from zipworkspace.desktop import selection
        real = selection.archive_sha
        def advance_after_hash(*args, **kw):
            digest = real(*args, **kw)
            with worktree.locked(self.state, live=self.live) as (con, _, __):
                con.execute("UPDATE checkout SET generation=2, parent=?", ("e"*32,))
            return digest
        with patch.object(selection, "archive_sha", side_effect=advance_after_hash):
            with self.assertRaisesRegex(Rejected, "原登记基线变化"):
                read_registered_baseline(self.live, self.baseline_display(), observer=Verification())

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop")
    def test_explicit_baseline_button_opens_only_a_read_only_view_in_another_window(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import DesktopWindows
        app = QApplication.instance() or QApplication([])
        windows = DesktopWindows()
        main = windows.open()
        pane = main.archive.checkout
        original_view = Selection.open(self.loose, retain=True)
        try:
            main.archive.show_selection(original_view)
            pane._attach(self.live)
            pane.live_record = self.live.status()
            pane._worktree_actions()

            def begin():
                future = Future()
                with patch.object(main.pool, "submit", return_value=future) as queued:
                    pane.browse.click()
                self.assertIs(main.pending[0], future)
                return future, queued.call_args.args[0]

            future, work = begin()
            view, error = work()
            self.assertIsNone(error)
            future.set_result((view, None))
            main.transfer_cancel.click(); main.poll()
            self.assertEqual(len(windows.windows), 1)
            self.assertTrue(view.held.closed)

            future, work = begin()
            future.set_result(work()); main.poll()
            self.assertIs(main.archive.selection, original_view)
            self.assertIs(pane.live, self.live)
            self.assertTrue(self.live.branch_source_available)
            child, = (win for win in windows.windows.values() if win is not main)
            self.assertEqual(child.archive.selection.exact.marker, self.first)
            self.assertIsNone(child.archive.store)
            self.assertIsNone(child.archive.checkout.live)
        finally:
            if main.pending is not None:
                main.pending[0].cancel(); main.poll()
            for win in tuple(windows.windows.values()): win.close()
            original_view.release_display()

    def test_changes_rejects_a_repacked_base_even_with_the_same_tree_and_marker(self):
        self.original.rename(self.root/"kept-original.zxs")
        snapshot(self.original, self.first, year=1981)
        with patch.object(lifecycle, "observe_held", side_effect=AssertionError("no scan from the wrong base")):
            with self.assertRaisesRegex(Rejected, "整包 SHA-256 不符"):
                lifecycle.observe_live_changes(self.live)

    def test_same_bytes_at_the_original_leaf_or_the_work_dir_are_not_adopted_mid_scan(self):
        real = lifecycle.observe_held
        archive_bytes = self.original.read_bytes()
        for entry in (self.original, self.tree):
            saved = self.root/("saved-" + entry.name)
            def substitute(*args, **kw):
                seen = real(*args, **kw)
                entry.rename(saved)
                if entry == self.tree: entry.mkdir()  # Same name, a different directory.
                else: entry.write_bytes(archive_bytes)  # Same exact bytes, a different inode.
                return seen
            with self.subTest(entry=entry), patch.object(lifecycle, "observe_held", side_effect=substitute):
                with self.assertRaises(Rejected): lifecycle.observe_live_changes(self.live)
            self.assertTrue(saved.exists()); self.assertTrue(entry.exists())
            if entry == self.tree: entry.rmdir()
            else: entry.unlink()
            saved.rename(entry)

    def test_initial_status_and_local_changes_do_not_touch_offline_store_or_loose_source(self):
        from zipworkspace.desktop import live_checkout
        self.store.rename(self.root/"offline-store")
        self.loose.rename(self.root/"former-selection.zxs")
        (self.tree/"story").write_bytes(b"offline edit")
        (self.tree/"local-empty").mkdir()
        original_named, real_stat = live_checkout._named_mount, os.fstat
        def named(path, fd):
            self.assertNotEqual(path, self.store)
            return original_named(path, fd)
        def fstat(fd):
            self.assertNotEqual(fd, self.live.store_fd)
            return real_stat(fd)
        with patch.object(live_checkout, "_named_mount", side_effect=named), \
             patch.object(os, "fstat", side_effect=fstat), \
             patch.object(lifecycle, "relative_document", side_effect=AssertionError("no archive reopened")), \
             patch.object(os, "fsync", side_effect=AssertionError("no writing")):
            record = self.live.status()
            before, report = lifecycle.observe_initial_changes(self.live)
        self.assertEqual(record["base_operation"], self.first.operation)
        self.assertEqual(before, self.first)
        self.assertEqual([(r.path, r.change) for r in report.rows],
                         [("local-empty", "added"), ("story", "changed")])
        with self.assertRaises(OSError): lifecycle.observe_live_changes(self.live)
        with patch.object(worktree.sqlite3, "connect", side_effect=AssertionError("no offline writer")):
            with self.assertRaisesRegex(Rejected, "only this live session's local read"):
                with worktree.locked(self.state, live=self.live, local_only=True): pass

    def test_offline_compare_requires_the_exact_initial_raw_digest_before_and_after_read(self):
        real_observe = lifecycle.observe_held
        def change_basis(*args, **kwargs):
            seen = real_observe(*args, **kwargs)
            with worktree.locked(self.state) as (con, _, __):
                con.execute("UPDATE checkout SET base_sha=? WHERE singleton=1", ("7"*64,))
            return seen
        with patch.object(lifecycle, "observe_held", side_effect=change_basis):
            with self.assertRaisesRegex(Rejected, "原基线改变"):
                lifecycle.observe_initial_changes(self.live)

    def test_can_stop_work_observation_in_both_modes_without_any_candidate(self):
        (self.tree/"story").write_bytes(b"z" * (CHUNK + 41))
        original_state = set(self.state.iterdir())

        class StopAtWork(Verification):
            def __init__(self):
                super().__init__()
                self.phases = []
            def begin(self, phase, total=None):
                super().begin(phase, total)
                self.phases.append(phase)
            def advance(self, n):
                super().advance(n)
                if self.phases[-1] == "folder": self.cancel()

        online = StopAtWork()
        with self.assertRaisesRegex(Rejected, "取消"):
            lifecycle.observe_live_changes(self.live, observer=online)
        self.assertEqual(online.phases, ["identity", "layout", "members", "identity", "raw", "folder"])
        offline = StopAtWork()
        with patch.object(lifecycle, "relative_document", side_effect=AssertionError("offline does not access STORE")), \
             self.assertRaisesRegex(Rejected, "取消"):
            lifecycle.observe_initial_changes(self.live, observer=offline)
        self.assertEqual(offline.phases, ["identity", "folder"])
        for watch in (online, offline):
            _, count, total = watch.progress()
            self.assertTrue(0 < count <= CHUNK)
            self.assertIsNone(total)  # no extra pre-scan or declaration for a mutable work directory
        self.assertEqual(set(self.state.iterdir()), original_state)
        self.assertIsNone(self.live.status()["pending_operation"])
        with patch.object(lifecycle, "observe_held", side_effect=AssertionError("wrong initial raw digest")):
            with self.assertRaisesRegex(Rejected, "不是首次签出的确切基线"):
                lifecycle.observe_initial_changes(self.live)

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_window_poll_uses_the_original_session_for_success_and_failure_and_releases_the_loan(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import checkout_pane
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        pane = win.archive.checkout
        native = lifecycle.observe_live_changes
        for stale, refused, cancelled in ((False, False, False), (False, True, False), (True, False, False),
                                          (True, True, False), (False, False, True)):
            live = self.open_live(self.loose, f"view-{stale}-{refused}-{cancelled}")
            pane.live = live
            pane.changes.setEnabled(True)
            jobs, loans = [], []
            def queue(work):
                future = Future()
                jobs.append((future, work))
                return future
            def read(loan, *, observer, retain_missing=False):
                loans.append(loan)
                self.assertIsNot(loan, live)
                if refused: raise Rejected("原读请求被拒")
                return native(loan, observer=observer, retain_missing=retain_missing)
            with self.subTest(stale=stale, refused=refused, cancelled=cancelled), \
                 patch.object(win.pool, "submit", side_effect=queue), \
                 patch.object(checkout_pane, "observe_live_changes", side_effect=read), \
                 patch.object(pane, "show_changes") as opened:
                pane.changes.click()
                future, work = jobs.pop()
                if stale:
                    pane.close_session()
                    win.inform("另一选择的提示")
                self.assertTrue(future.set_running_or_notify_cancel())
                future.set_result(work())
                if cancelled: win.transfer_cancel.click()
                win.poll()
                self.assertIsNone(win.pending)
                self.assertTrue(loans[0].closed)
                if stale:
                    opened.assert_not_called()
                    self.assertEqual(win.note.text(), "另一选择的提示")
                elif cancelled:
                    opened.assert_not_called()
                    self.assertIn("已取消工作树对照", win.note.text())
                elif refused:
                    opened.assert_not_called()
                    self.assertIn("原读请求被拒", win.note.text())
                else:
                    opened.assert_called_once()
                    observed_live, marker, report = opened.call_args.args
                    self.assertIs(observed_live, live)
                    self.assertEqual(marker, self.first)
                    self.assertEqual(report.rows, ())
                pane.close_session()


    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_window_links_offline_status_and_the_initial_comparison_button(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        pane = win.archive.checkout; pane._attach(self.live)
        self.store.rename(self.root/"offline")
        (self.tree/"story").write_bytes(b"changed while offline")
        jobs = []
        def queue(work):
            future = Future()
            jobs.append((future, work))
            return future
        def poll():
            future, work = jobs.pop(0)
            self.assertTrue(future.set_running_or_notify_cancel())
            future.set_result(work())
            win.poll()
        with patch.object(win.pool, "submit", side_effect=queue):
            self.assertFalse(pane.initial_changes.isEnabled())
            pane.refresh_live(); poll()
            self.assertTrue(pane.initial_changes.isEnabled())
            pane.initial_changes.click(); poll()
            self.assertEqual(pane.changes_dialog.windowTitle(), "离线对照首次签出版本")
            self.assertIn("未检查 STORE", win.note.text())
            pane.changes_dialog.reject()
            self.assertFalse(jobs)

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_worktree_dialog_pages_full_relative_names_and_closes_with_the_session(self):
        from PySide6.QtWidgets import QApplication, QPushButton
        from zipworkspace.desktop.widgets import Table
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        pane = win.archive.checkout; pane.live = self.live
        for i in range(101): (self.tree/f"file-{i:03}").write_bytes(b"")
        marker, report = lifecycle.observe_live_changes(self.live)
        pane.show_changes(self.live, marker, report)
        dialog = pane.changes_dialog
        table = dialog.findChild(Table)
        pages = {b.text(): b for b in dialog.findChildren(QPushButton)}
        def names():
            return [row.path for _, row in table.rows.rows]
        self.assertTrue(dialog.isModal())
        self.assertEqual(names(), [f"file-{i:03}" for i in range(100)])
        pages["下一页"].click()
        self.assertEqual(names(), ["file-100"])
        self.assertFalse(pages["下一页"].isEnabled())
        pages["上一页"].click()
        self.assertEqual(names(), [f"file-{i:03}" for i in range(100)])
        pane.close_session()
        self.assertIsNone(pane.changes_dialog)
        self.assertFalse(dialog.isVisible())


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

    def test_changes_uses_the_current_committed_base_not_the_loose_checkout_and_holds_no_read_lock_during_scan(self):
        story = self.tree/"story"
        story.write_bytes(b"revised  story")
        committed_path, marker = self.choice().run()
        story.write_bytes(b"original story")  # Same size as revision two, but equal to revision one.
        (self.tree/"empty").rmdir()
        (self.tree/"new-empty").mkdir()
        self.loose.rename(self.root/"original-loose-kept.zxs")
        initial_status = self.live.status()
        self.assertEqual(self.live.checkout_marker, self.first)
        opened, active = [], []
        real_locked, real_observe = worktree.locked, lifecycle.observe_held
        @contextmanager
        def read_only(state, *, read_only, live, local_only=False):
            self.assertEqual((state, read_only, live, local_only), (self.state, True, self.live, False))
            with real_locked(state, read_only=read_only, live=live) as data:
                opened.append(data[1]["parent"]); active.append(True)
                try: yield data
                finally: active.pop()
        def observe(*args, **kwargs):
            self.assertFalse(active)
            self.assertEqual(args[0], self.live.work_fd)
            return real_observe(*args, **kwargs)
        with patch.object(worktree, "locked", side_effect=read_only), \
             patch.object(lifecycle, "observe_held", side_effect=observe), \
             patch.object(worktree, "anonymous_capture", side_effect=AssertionError("no frozen copy")), \
             patch.object(worktree, "finish", side_effect=AssertionError("no publish/recovery")), \
             patch.object(os, "fsync", side_effect=AssertionError("read-only comparison")):
            base, report = lifecycle.observe_live_changes(self.live)
        self.assertEqual((base, opened), (marker, [marker.operation, marker.operation]))
        self.assertEqual((report.added, report.changed, report.missing), (1, 1, 1))
        self.assertEqual([(r.path, r.change, r.before, r.after) for r in report.page(0)], [
            ("empty", "missing", "directory", None), ("new-empty", "added", None, "directory"),
            ("story", "changed", "file", "file")])
        self.assertEqual([(r.before_bytes, r.after_bytes) for r in report.rows[-1:]], [(14, 14)])
        self.assertEqual(self.live.status(), initial_status)
        self.assertEqual(set(self.store.iterdir()), {self.original, committed_path})

    def test_changes_refuses_if_a_real_cli_commit_advances_the_base_during_the_scan(self):
        (self.tree/"story").write_bytes(b"edited")
        real = lifecycle.observe_held
        advanced = []
        def interleaved(*args, **kw):
            seen = real(*args, **kw)
            advanced.append(worktree.commit(self.state)[1].operation)
            return seen
        with patch.object(lifecycle, "observe_held", side_effect=interleaved):
            with self.assertRaisesRegex(Rejected, "基线改变"):
                lifecycle.observe_live_changes(self.live)
        self.assertEqual(len(advanced), 1)
        self.assertEqual(self.live.status()["base_operation"], advanced[0])

    def test_changes_rejects_an_existing_pending_without_reading_work_files(self):
        choice = self.choice()
        with patch.object(worktree, "_publish", side_effect=publish.Uncertain("reply lost")):
            with self.assertRaises(publish.Uncertain): choice.run()
        with patch.object(lifecycle, "observe_held", side_effect=AssertionError("no scan")):
            with self.assertRaisesRegex(Rejected, "无待决"):
                lifecycle.observe_live_changes(self.live)
        self.assertEqual(self.live.status()["pending_operation"], choice.operation)
        self.assertEqual(list(self.store.iterdir()), [self.original])

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

    def test_stop_during_held_worktree_freeze_never_creates_a_named_candidate(self):
        from zipworkspace.private_freeze import Capture
        choice = self.choice()
        genuine = Capture.append
        def append(capture, data):
            genuine(capture, data)
            self.assertIn("正在冻结本次选中的原工作目录", choice.preparation.progress()[0])
            self.assertTrue(choice.preparation.cancel())
        with patch.object(Capture, "append", new=append), \
             patch.object(worktree, "pack", side_effect=AssertionError("do not pack after stop")), \
             self.assertRaisesRegex(CommitStopped, "没有登记本次待决"):
            choice.run()
        self.assertFalse((self.state/(choice.operation+".zxs")).exists())
        self.assertIsNone(self.live.status()["pending_operation"])
        self.assertEqual(list(self.store.iterdir()), [self.original])

    def test_private_pack_completed_before_stop_does_not_authorize_publication(self):
        from zipworkspace.private_candidates import CandidateDirectory
        choice = self.choice()
        genuine = CandidateDirectory.verify_candidate
        def verify(area, leaf, fd, marker, **options):
            digest = genuine(area, leaf, fd, marker, **options)
            self.assertIn("同步私有候选", choice.preparation.progress()[0])
            self.assertTrue(choice.preparation.cancel())
            return digest
        with patch.object(CandidateDirectory, "verify_candidate", new=verify), \
             patch.object(worktree, "_publish", side_effect=AssertionError("do not touch STORE after stop")), \
             self.assertRaisesRegex(CommitStopped, "没有登记本次待决"):
            choice.run()
        self.assertTrue((self.state/(choice.operation+".zxs")).is_file())
        self.assertEqual(choice.result()[0], "unrecorded")
        self.assertIsNone(self.live.status()["pending_operation"])
        self.assertEqual(list(self.store.iterdir()), [self.original])

    def test_cannot_stop_after_original_pending_is_recorded_or_create_a_second_operation(self):
        choice = self.choice()
        publisher = worktree._publish
        seen = []
        def publish_with_stop(*args, **kwargs):
            self.assertEqual(self.live.status()["pending_operation"], choice.operation)
            self.assertFalse(choice.preparation.cancel())
            self.assertIn("现在不能撤销", choice.preparation.progress()[0])
            seen.append(choice.operation)
            return publisher(*args, **kwargs)
        with patch.object(worktree, "_publish", side_effect=publish_with_stop):
            output, marker = choice.run()
        self.assertEqual(marker.operation, choice.operation)
        self.assertEqual(seen, [choice.operation])
        self.assertTrue(output.is_file())
        self.assertEqual(choice.result()[0], "completed")

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_window_can_stop_during_read_review_or_before_freeze_and_keeps_the_last_real_receipt(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import commit_controls
        from zipworkspace.desktop.window import Window
        previous = self.choice()
        previous.run()
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        pane = win.archive.checkout.commits
        pane.set_session(self.live)
        pane.last = previous
        pane.buttons()
        for stage in ("late review", "first freeze"):
            with self.subTest(stage=stage):
                queued = []
                def execute(call):
                    future = Future()
                    queued.append((future, call))
                    return future
                with patch.object(win.pool, "submit", side_effect=execute), \
                     patch.object(commit_controls, "confirm", return_value=True) as confirm:
                    pane.save.click()
                    read, call = queued[0]
                    read.set_result(call())
                    self.assertFalse(win.tabs.isEnabled()); self.assertTrue(win.transfer_cancel.isEnabled())
                    if stage == "late review":
                        win.transfer_cancel.click()
                        win.poll()
                        confirm.assert_not_called()
                        self.assertEqual(len(queued), 1)
                    else:
                        win.poll()
                        confirm.assert_called_once()
                        write, call = queued[1]
                        self.assertFalse(win.tabs.isEnabled()); self.assertTrue(win.transfer_cancel.isEnabled())
                        win.transfer_cancel.click()
                        with self.assertRaises(CommitStopped) as stopped: call()
                        write.set_exception(stopped.exception)
                        win.poll()
                self.assertIs(pane.last, previous)
                self.assertTrue(pane.save.isEnabled())
                self.assertIn("没有登记本次待决", pane.output.text())
                self.assertIsNone(self.live.status()["pending_operation"])
                self.assertEqual(self.live.status()["base_operation"], previous.operation)

    def test_original_pending_directory_barrier_precedes_any_store_attempt_and_exact_retry(self):
        original = self.choice()
        real_sync, real_publish = os.fsync, worktree._publish
        saw_pending = []

        def sync(fd):
            if (os.path.samestat(os.fstat(fd), os.fstat(self.live.state_fd))
                    and self.live.status()["pending_operation"] == original.operation):
                self.assertTrue((self.state/(original.operation+".zxs")).is_file())
                saw_pending.append(original.operation)
                raise OSError(errno.ENOSPC, "pending directory barrier")
            return real_sync(fd)

        with patch.object(os, "fsync", side_effect=sync), \
             patch.object(worktree, "_publish", side_effect=AssertionError("do not attempt STORE before STATE")), \
             self.assertRaisesRegex(OSError, "pending directory barrier") as failed:
            original.run()
        self.assertEqual(saw_pending, [original.operation])
        self.assertIn("本次尚未尝试上传或核认正式槽", "\n".join(failed.exception.__notes__))
        self.assertEqual(list(self.store.iterdir()), [self.original])
        frozen = self.state/(original.operation+".zxs")
        self.assertTrue(frozen.is_file())
        retry = self.retry_choice(original); retry.prepare()
        ordered = []

        def pending_sync(fd):
            if os.path.samestat(os.fstat(fd), os.fstat(self.live.state_fd)):
                readback = self.live.status()
                if readback["pending_operation"] == original.operation:
                    ordered.append("STATE")
                else:
                    self.assertEqual(readback["base_operation"], original.operation)
                    ordered.append("RECEIPT")
            return real_sync(fd)

        def publish_original(*args, **options):
            self.assertEqual(ordered, ["STATE"])
            self.assertEqual(args[2], original.marker)
            ordered.append("STORE")
            return real_publish(*args, **options)

        with patch.object(os, "fsync", side_effect=pending_sync), \
             patch.object(worktree, "_publish", side_effect=publish_original), \
             patch.object(worktree, "frozen_tree", side_effect=AssertionError("no recapture")):
            final, marker = retry.run()
        self.assertEqual(ordered, ["STATE", "STORE", "RECEIPT"])
        self.assertEqual(final.read_bytes(), frozen.read_bytes())
        self.assertEqual(marker, original.marker)

    def test_non_delete_journal_never_produces_a_new_frozen_pending_in_the_window(self):
        import sqlite3
        db = self.state/"worktree.sqlite"
        with sqlite3.connect(db) as con:
            self.assertEqual(con.execute("PRAGMA journal_mode=WAL").fetchone()[0], "wal")
        con.close()
        original = self.choice()
        with patch.object(worktree, "frozen_tree", side_effect=AssertionError("wrong SQLite journal")), \
             patch.object(worktree, "_publish", side_effect=AssertionError("no STORE attempt")), \
             self.assertRaisesRegex(Rejected, "DELETE 回滚日志"):
            original.run()
        self.assertFalse((self.state/(original.operation+".zxs")).exists())
        self.assertEqual(self.live.status()["pending_operation"], None)
        with sqlite3.connect(db) as check:
            self.assertEqual(check.execute("PRAGMA journal_mode").fetchone()[0], "wal")
        check.close()

    def lose_directory_receipt(self, original, invocation):
        real = os.fsync
        def sync(fd):
            if (os.path.samestat(os.fstat(fd), os.fstat(self.live.state_fd))
                    and self.live.status()["base_operation"] == original.operation):
                raise OSError(errno.EIO, "final original receipt barrier")
            return real(fd)
        with patch.object(os, "fsync", side_effect=sync), self.assertRaisesRegex(OSError, "final original receipt barrier"):
            invocation()
        target = self.store/original.marker.filename
        self.assertEqual(target.read_bytes(), (self.state/(original.operation+".zxs")).read_bytes())
        self.assertEqual(self.live.status()["base_operation"], original.operation)
        self.assertIsNone(self.live.status()["pending_operation"])

    def test_late_local_receipt_barrier_remains_uncertain_until_explicit_local_only_confirmation(self):
        original = self.choice()
        self.lose_directory_receipt(original, original.run)
        self.store.rename(self.root/"offline-store")
        with patch.object(os, "fsync", side_effect=AssertionError("query does not sync")):
            self.assertEqual(original.result()[0], "receipt_dir")
            with self.assertRaisesRegex(Rejected, "明确确认"): original.confirm_local_receipt()
        self.assertTrue(original.local_receipt_seen)
        with patch.object(worktree, "_publish", side_effect=AssertionError("nothing to transmit or read from STORE")), \
             patch.object(worktree, "finish", side_effect=AssertionError("no writing transaction")):
            self.assertIn("没有接触 STORE", original.confirm_local_receipt(confirmed=True))
        self.assertEqual(original.result()[0], "completed")
        self.assertFalse(original.local_receipt_seen)

    def test_exact_retry_keeps_its_late_sync_failure_on_the_original_window_operation(self):
        original = self.pending_choice()
        retry = self.retry_choice(original); retry.prepare()
        self.lose_directory_receipt(original, retry.run)
        self.assertIs(retry.receipt, original.receipt)
        self.assertEqual(original.result()[0], "receipt_dir")
        self.assertFalse(original.pending_seen)
        with self.assertRaises(Rejected): PendingRetry(original)

    def test_visible_row_after_a_lost_sql_reply_never_grants_a_directory_only_recovery(self):
        original = self.choice()
        opened = worktree.locked
        @contextmanager
        def lose_reply(state, **options):
            with opened(state, **options) as value:
                yield value
            if options.get("phase", "").startswith("记原提交回执"):
                raise OSError("lost end of SQLite writer")
        with patch.object(worktree, "locked", side_effect=lose_reply), \
             self.assertRaisesRegex(OSError, "lost end of SQLite writer"):
            original.run()
        self.assertTrue((self.store/original.marker.filename).is_file())
        self.assertEqual(original.result()[0], "completion_unsynced")
        self.assertFalse(original.local_receipt_seen)
        with self.assertRaises(Rejected): original.confirm_local_receipt(confirmed=True)

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_window_local_receipt_is_a_separate_explicit_choice_after_its_read_only_query(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import commit_controls
        from zipworkspace.desktop.window import Window
        original = self.choice()
        self.lose_directory_receipt(original, original.run)
        self.store.rename(self.root/"offline-store")
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        controls = win.archive.checkout.commits
        controls.set_session(self.live)
        controls.last, controls.blocked = original, True
        controls.buttons()
        self.assertFalse(controls.sync_receipt.isEnabled())

        def dispatch(_title, work, apply, *, failed):
            try: outcome = work()
            except Exception as err: failed(err)
            else: apply(outcome)
            return object()

        with patch.object(win, "job", side_effect=dispatch), patch.object(controls, "refresh_status"), \
             patch.object(commit_controls, "confirm", side_effect=[False, True]) as confirm:
            controls.inspect()
            self.assertTrue(controls.sync_receipt.isEnabled())
            with patch.object(os, "fsync", side_effect=AssertionError("no sync if declined")):
                controls.sync_receipt.click()
            self.assertTrue(original.receipt.directory_remaining)
            self.assertFalse(controls.save.isEnabled())
            controls.sync_receipt.click()
        self.assertEqual(confirm.call_count, 2)
        self.assertFalse(controls.sync_receipt.isEnabled())
        self.assertTrue(controls.save.isEnabled())
        self.assertIn("没有接触 STORE", controls.output.text())

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
            def immediate(_title, fn, apply, *, failed=None, **options):
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
        def immediate(_title, call, result, *, failed=None, **options):
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


class FrozenConflictBranch(OriginalCheckoutFixture):
    needs_freeze = True

    def conflicted(self, mode="valid"):
        (self.tree/"story").write_bytes(b"losing attempt, already frozen")
        original = CommitChoice(self.live); self.addCleanup(original.release)
        original.prepare()
        winner = replace(self.first, generation=2, operation="d"*32,
                         parent="e"*32 if mode == "wrong-parent" else self.first.operation)
        slot = self.store/winner.filename
        if mode == "corrupt":
            slot.write_bytes(b"not a ZIP, but bytes differ from the original")
        else:
            snapshot(slot, winner)
        with self.assertRaises(publish.Conflict): original.run()
        self.assertFalse(original.conflict_pending_seen)  # A publish error is not the requested readback.
        self.assertEqual(original.result()[0], "pending")
        self.assertTrue(original.conflict_pending_seen)
        self.assertFalse(original.pending_seen)
        with self.assertRaises(Rejected): PendingRetry(original)
        return original, slot, winner

    def review_fork(self, original):
        attempt = PendingFork(original); self.addCleanup(attempt.release)
        text = attempt.prepare()
        return attempt, text

    def test_confirmed_fork_keeps_exact_losing_tree_and_never_opens_live_edits(self):
        original, slot, winner = self.conflicted()
        old_path = self.state/(original.operation+".zxs")
        old_bytes, winner_bytes = old_path.read_bytes(), slot.read_bytes()
        (self.tree/"story").write_bytes(b"later editor changes")
        self.live.revoke_branch_source()  # No dependency on the initial browser selection.
        with patch.object(os, "fsync", side_effect=AssertionError("prepare is read-only")):
            attempt, detail = self.review_fork(original)
        for value in (original.operation, original.digest, winner.operation, attempt.operation,
                      attempt.proposed.branch, self.store/attempt.proposed.filename):
            self.assertIn(str(value), detail)
        with patch.object(worktree, "locked", side_effect=AssertionError("unconfirmed writer")):
            with self.assertRaises(Rejected):
                worktree.fork_pending(self.state, original.operation, selected=attempt)
        with patch.object(worktree, "anonymous_capture", side_effect=AssertionError("must not freeze")), \
             patch.object(worktree, "frozen_tree", side_effect=AssertionError("must not scan worktree")), \
             patch.object(worktree, "publish", side_effect=AssertionError("must publish through held STORE")):
            path, marker = attempt.run()
        self.assertEqual((marker.operation, marker.parent, marker.tree, marker.generation),
                         (attempt.operation, original.marker.parent, original.marker.tree, 1))
        self.assertEqual((path, marker.branch), (self.store/attempt.proposed.filename, attempt.proposed.branch))
        self.assertNotEqual(marker.branch, original.marker.branch)
        with zipfile.ZipFile(path) as doc:
            self.assertEqual(doc.read("story"), b"losing attempt, already frozen")
            self.assertIn("empty/", doc.namelist())
        self.assertEqual((self.tree/"story").read_bytes(), b"later editor changes")
        self.assertEqual((old_path.read_bytes(), slot.read_bytes()), (old_bytes, winner_bytes))
        self.assertEqual(self.live.status()["base_operation"], attempt.operation)
        self.assertEqual(original.result()[0], "forked")
        self.assertEqual(attempt.result()[0], "completed")
        with self.assertRaises(Rejected): attempt.run()

    def reject_bad_winner(self, mode):
        original, slot, _ = self.conflicted(mode)
        before = {p.name for p in self.state.iterdir()}
        saved = slot.read_bytes()
        attempt = PendingFork(original); self.addCleanup(attempt.release)
        with patch.object(os, "fsync", side_effect=AssertionError("no output for an invalid winner")), \
             self.assertRaises((Rejected, zipfile.BadZipFile)):
            attempt.prepare()
        self.assertTrue(attempt.closed)
        with self.assertRaises(Rejected): attempt.run()
        self.assertEqual({p.name for p in self.state.iterdir()}, before)
        self.assertEqual(slot.read_bytes(), saved)
        self.assertEqual(self.live.status()["pending_operation"], original.operation)

    def test_different_bytes_without_valid_zip_are_not_a_conflict_authorization(self):
        self.reject_bad_winner("corrupt")

    def test_valid_zip_from_the_wrong_parent_is_not_this_conflict(self):
        self.reject_bad_winner("wrong-parent")

    def test_equal_byte_replacements_of_both_reviewed_names_are_rejected(self):
        original, slot, _ = self.conflicted()
        for leaf in (self.state/(original.operation+".zxs"), slot):
            with self.subTest(name=leaf.name):
                self.assertEqual(original.result()[0], "pending")
                attempt, _ = self.review_fork(original)
                content = leaf.read_bytes()
                held = self.root/("held-"+leaf.name); leaf.rename(held)
                leaf.write_bytes(content)
                with patch.object(worktree, "locked", side_effect=AssertionError("cannot reach a writer")), \
                     self.assertRaises(Rejected):
                    attempt.run()
                leaf.rename(self.root/("replacement-"+leaf.name))
                held.rename(leaf)
                self.assertEqual(attempt.result()[0], "unaccepted")
                self.assertFalse((self.state/(attempt.operation+".zxs")).exists())

    def test_an_intervening_different_successor_is_not_adopted(self):
        original, slot, _ = self.conflicted()
        attempt, _ = self.review_fork(original)
        with patch.object(worktree, "finish", side_effect=OSError("CLI intent only")), self.assertRaises(OSError):
            worktree.fork_pending(self.state, original.operation)  # Deliberate separate CLI decision.
        successor = self.live.status()["pending_operation"]
        self.assertNotIn(successor, (original.operation, attempt.operation))
        with patch.object(worktree, "_publish", side_effect=AssertionError("do not follow another successor")), \
             self.assertRaises(Rejected):
            attempt.run()
        self.assertEqual(attempt.result()[0], "other")
        self.assertFalse((self.state/(attempt.operation+".zxs")).exists())
        self.assertCountEqual(self.store.iterdir(), (self.original, slot))

    def test_new_slot_collision_never_replaces_or_picks_a_different_name(self):
        original, slot, _ = self.conflicted()
        attempt, _ = self.review_fork(original)
        new_leaf = self.store/attempt.proposed.filename
        new_leaf.write_bytes(b"reserved by someone else")
        with self.assertRaises(publish.Conflict): attempt.run()
        self.assertEqual(new_leaf.read_bytes(), b"reserved by someone else")
        self.assertEqual(self.live.status()["pending_operation"], original.operation)
        self.assertEqual(attempt.result()[0], "unaccepted")
        self.assertFalse((self.state/(attempt.operation+".zxs")).exists())
        self.assertCountEqual(self.store.iterdir(), (self.original, slot, new_leaf))

    def test_lost_reply_after_durable_acceptance_only_observes_the_original_pair(self):
        original, _, _ = self.conflicted()
        attempt, _ = self.review_fork(original)
        with patch.object(worktree, "finish", side_effect=publish.Uncertain("owner stops after SQLite COMMIT")), \
             self.assertRaises(publish.Uncertain): attempt.run()
        self.assertEqual(self.live.status()["pending_operation"], attempt.operation)
        self.assertFalse((self.store/attempt.proposed.filename).exists())
        with patch.object(worktree, "finish", side_effect=AssertionError("receipt query cannot resume")), \
             patch.object(os, "fsync", side_effect=AssertionError("receipt query cannot sync")):
            kind, text = attempt.result()
        self.assertEqual(kind, "pending")
        self.assertIn(attempt.operation, text); self.assertIn("不能当作完成", text)
        self.assertEqual(original.result()[0], "forked")

    def test_losing_inputs_are_no_longer_required_after_durable_successor_acceptance(self):
        original, slot, _ = self.conflicted()
        attempt, _ = self.review_fork(original)
        old_private = self.state/(original.operation+".zxs")
        real = worktree.finish
        def originals_moved(state, operation, *, selected=None):
            for proof in (slot, old_private):
                proof.rename(self.root/("moved-"+proof.name))
            return real(state, operation, selected=selected)
        with patch.object(worktree, "finish", side_effect=originals_moved):
            target, marker = attempt.run()
        self.assertEqual((target, marker), (self.store/attempt.proposed.filename, attempt.proposed))
        self.assertEqual(attempt.result()[0], "completed")

    def test_new_slot_claimed_after_acceptance_keeps_exact_successor_pending(self):
        original, _, _ = self.conflicted()
        attempt, _ = self.review_fork(original)
        formerly_empty = self.store/attempt.proposed.filename
        real = worktree.finish
        def claim(state, operation, *, selected=None):
            formerly_empty.write_bytes(b"other data at the accepted successor slot")
            return real(state, operation, selected=selected)
        with patch.object(worktree, "finish", side_effect=claim), self.assertRaises(publish.Conflict):
            attempt.run()
        self.assertEqual(formerly_empty.read_bytes(), b"other data at the accepted successor slot")
        self.assertEqual(self.live.status()["pending_operation"], attempt.operation)
        self.assertEqual(attempt.result()[0], "pending")

    def test_completed_by_another_client_between_transactions_never_runs_store_readback_twice(self):
        original, _, _ = self.conflicted()
        attempt, _ = self.review_fork(original)
        real = worktree.finish
        def concurrent(state, operation, *, selected=None):
            real(state, operation)  # The same already accepted successor finished via CLI.
            with patch.object(publish.StoreReadback, "confirm", side_effect=AssertionError("completed must only be queried")):
                return real(state, operation, selected=selected)
        with patch.object(worktree, "finish", side_effect=concurrent), self.assertRaisesRegex(Rejected,"已登记完成"):
            attempt.run()
        self.assertEqual(attempt.result()[0], "completed")
        self.assertEqual(self.live.status()["base_operation"], attempt.operation)

    def test_visible_store_file_without_receipt_is_pending_and_later_only_confirms_that_successor(self):
        original, _, _ = self.conflicted()
        attempt, _ = self.review_fork(original)
        real = worktree._publish
        def lost(*args, **kw):
            real(*args, **kw)
            raise publish.Uncertain("final archive is visible, original receipt not recorded")
        with patch.object(worktree, "_publish", side_effect=lost), self.assertRaises(publish.Uncertain):
            attempt.run()
        target = self.store/attempt.proposed.filename
        seen = target.stat()
        self.assertEqual(attempt.result()[0], "pending")
        with patch.object(publish, "_new_upload", side_effect=AssertionError("no second upload")):
            returned, marker = worktree.finish(self.state, attempt.operation)  # Outside GUI; explicit original ID.
        self.assertEqual((returned, marker), (target, attempt.proposed))
        self.assertTrue(os.path.samestat(seen, target.stat()))
        self.assertEqual(attempt.result()[0], "completed")

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_window_cancel_and_a_stale_confirmation_cannot_publish_then_fresh_confirmation_runs_once(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import commit_controls
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        controls = win.archive.checkout.commits
        original, slot, _ = self.conflicted()
        def immediate(_title, call, accept, *, failed=None):
            try: answer = call()
            except Exception as error:
                if failed is None: raise
                failed(error)
            else: accept(answer)
            return object()
        controls.set_session(self.live); controls.last, controls.blocked = original, True
        controls.buttons()
        self.assertTrue(controls.conflict_fork.isEnabled()); self.assertFalse(controls.retry.isEnabled())
        def stale(*args, **kw):
            win.pending = object()  # An intervening event during the actual confirmation.
            return True
        with patch.object(win, "job", side_effect=immediate), patch.object(controls, "refresh_status"), \
             patch.object(worktree, "fork_pending", side_effect=AssertionError("not authorized")):
            with patch.object(commit_controls, "confirm", return_value=False):
                controls.conflict_fork.click()
            self.assertFalse(controls.conflict_fork.isEnabled())
            self.assertIs(controls.last, original)
            controls.query.click()
            try:
                with patch.object(commit_controls, "confirm", side_effect=stale):
                    controls.conflict_fork.click()
            finally: win.pending = None
        self.assertCountEqual(self.store.iterdir(), (self.original, slot))
        def approve(_parent, _question, text, *, attestation, action_text):
            self.assertIn(original.operation, text); self.assertIn("不发布冻结后的编辑", attestation)
            self.assertEqual(action_text, "发布原候选到指定新分支")
            self.assertFalse(controls.conflict_fork.isEnabled())
            controls.conflict_fork.click()
            return True
        with patch.object(win, "job", side_effect=immediate), patch.object(controls, "refresh_status"), \
             patch.object(commit_controls, "confirm", side_effect=approve) as ask:
            controls.query.click()
            controls.conflict_fork.click()
            self.assertEqual(ask.call_count, 1)
        self.assertIsInstance(controls.last, PendingFork)
        self.assertIn(original.operation, controls.output.text())
        self.assertIn(controls.last.operation, controls.output.text())
        self.assertFalse(controls.retry.isEnabled())
        self.assertEqual(self.live.status()["pending_operation"], None)


class RegisteredNodeBranch(OriginalCheckoutFixture):
    needs_freeze = True

    def registration(self):
        r = self.live.status()
        return tuple(r[k] for k in ("repo", "branch", "base_generation", "base_operation"))

    def prepare_branch(self):
        branch = SnapshotBranch(self.live, registered=self.registration())
        self.addCleanup(branch.release)
        detail = branch.prepare()
        self.assertIn("所列的登记基线", detail)
        return branch

    def test_a_later_registered_commit_can_be_forked_after_losing_the_original_display(self):
        (self.tree/"story").write_bytes(b"only the new committed version")
        save = CommitChoice(self.live); self.addCleanup(save.release)
        save.prepare()
        canonical, current = save.run()
        before = canonical.read_bytes()
        self.assertEqual(current.generation, 2)
        self.live.revoke_branch_source()
        self.loose.rename(self.root/"unavailable-first-display.zxs")
        with self.assertRaises(Rejected): SnapshotBranch(self.live)
        (self.tree/"story").write_bytes(b"keep this unsubmitted edit")
        with patch.object(worktree, "frozen_tree", side_effect=AssertionError("branch does not read work content")):
            branch = self.prepare_branch()
            self.assertEqual(branch.proposed.parent, current.operation)
            final, marker = branch.run()
        self.assertEqual((marker.parent, marker.tree, marker.generation),
                         (current.operation, current.tree, 1))
        self.assertNotEqual(marker.branch, current.branch)
        self.assertEqual(canonical.read_bytes(), before)
        self.assertEqual((self.tree/"story").read_bytes(), b"keep this unsubmitted edit")
        with zipfile.ZipFile(final) as original:
            self.assertEqual(original.read("story"), b"only the new committed version")
        self.assertEqual(self.registration(), (marker.repo, marker.branch, 1, marker.operation))

    def test_registered_branch_checks_the_display_before_opening_any_archive_and_refuses_an_orphan_sha(self):
        from zipworkspace.desktop import commit_choice
        before = set(self.store.iterdir())
        r = self.registration()
        stale = SnapshotBranch(self.live, registered=(r[0], r[1], 2, r[3]))
        self.addCleanup(stale.release)
        with patch.object(commit_choice, "relative_document", side_effect=AssertionError("different source")), \
             self.assertRaisesRegex(Rejected, "界面|窗口状态"):
            stale.prepare()
        actual = self.prepare_branch()
        with worktree.locked(self.state) as (con, _, __):
            con.execute("UPDATE checkout SET pending_sha=?", ("f"*64,))
        with patch.object(actual.area, "copy_with_marker", side_effect=AssertionError("must refuse before write")), \
             self.assertRaises(Rejected):
            actual.run()
        self.assertEqual(set(self.store.iterdir()), before)
        self.assertFalse((self.state/(actual.operation+".zxs")).exists())

    def test_a_persisted_registered_branch_retries_only_the_original_operation(self):
        self.live.revoke_branch_source()
        branch = self.prepare_branch()
        with patch.object(worktree, "_publish", side_effect=OSError(errno.EIO, "failed original upload")), \
             self.assertRaises(OSError):
            branch.run()
        self.assertEqual(branch.result()[0], "pending")
        retry = PendingRetry(branch); self.addCleanup(retry.release)
        self.assertTrue(retry.registered_source)
        self.assertIn("登记完成", retry.prepare())
        self.assertEqual(retry.operation, branch.operation)
        (self.tree/"story").write_bytes(b"later edits")
        with patch.object(worktree, "frozen_tree", side_effect=AssertionError("no refreeze on retry")):
            final, marker = retry.run()
        self.assertEqual(marker, branch.proposed)
        self.assertTrue(final.is_file())
        self.assertEqual((self.tree/"story").read_bytes(), b"later edits")

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop")
    def test_registered_branch_button_requires_a_current_explicit_confirmation(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import commit_controls
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        self.live.revoke_branch_source()
        try:
            pane = win.archive.checkout
            pane._attach(self.live)
            pane.live_record = self.live.status()
            pane._worktree_actions()
            controls = pane.commits
            self.assertFalse(controls.new_branch.isEnabled())
            self.assertTrue(controls.base_branch.isEnabled())
            future = Future()
            with patch.object(win.pool, "submit", return_value=future) as queued:
                controls.base_branch.click()
            future.set_result(queued.call_args.args[0]())

            def changed(_parent, question, detail, **_):
                self.assertIn("登记基线", question)
                self.assertIn(self.first.operation, detail)
                r = self.registration()
                controls.allow_visible(True, registered=(r[0], r[1], 2, r[3]))
                return True

            with patch.object(commit_controls, "confirm", side_effect=changed), \
                 patch.object(win.pool, "submit", side_effect=AssertionError("no write after the decision changed")):
                win.poll()
            self.assertIsNone(controls.last)
            self.assertEqual(self.live.status()["pending_operation"], None)
            self.assertCountEqual(self.store.iterdir(), [self.original])
        finally:
            if win.pending is not None:
                win.pending[0].cancel(); win.poll()
            win.close()


class DisplayedNodeBranch(OriginalCheckoutFixture):
    def proposal(self, live=None):
        job = SnapshotBranch(live or self.live); self.addCleanup(job.release)
        return job

    def test_leaving_the_original_view_revokes_fresh_and_already_prepared_branch_choices(self):
        unprepared, prepared = self.proposal(), self.proposal()
        prepared.prepare()
        self.live.revoke_branch_source()
        with patch.object(worktree, "locked", side_effect=AssertionError("refuse before SQLite")), \
             patch.object(worktree, "branch_from_snapshot", side_effect=AssertionError("refuse before writes")):
            with self.assertRaisesRegex(Rejected, "已离开签出时的原浏览快照"):
                self.proposal()
            with self.assertRaisesRegex(Rejected, "已离开签出时的原浏览快照"):
                unprepared.prepare()
            with self.assertRaisesRegex(Rejected, "已离开签出时的原浏览快照"):
                prepared.run()
        self.assertFalse(prepared.used)
        self.assertIsNone(self.live.status()["pending_operation"])
        self.assertEqual(list(self.store.iterdir()), [self.original])

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_browsing_retains_one_original_session_pending_receipt_and_explicit_close_changes_no_files(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import checkout_pane
        from zipworkspace.desktop.selection import StoreSelection
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        archive, pane = win.archive, win.archive.checkout
        source = Selection.open(self.loose, retain=True)
        self.addCleanup(source.release_display)
        archive.show_selection(source)

        def actual_checkout(leaf):
            plan = CheckoutChoice(source, keep_session=True)
            self.addCleanup(plan.close)
            plan.pick_store(self.store)
            plan.pick_target(self.tree.parent)
            plan.pick_private(self.state.parent)
            plan.review(leaf)
            result = plan.run()
            return plan, result

        plan, first = actual_checkout("Window")
        with patch.object(pane, "refresh_live"): pane._complete(plan, first)
        live, controls = pane.live, pane.commits
        self.assertIsNotNone(live)
        self.assertTrue(controls.new_branch.isEnabled())
        self.assertFalse(pane.start.isEnabled())
        self.assertTrue(pane.end.isEnabled())
        job = self.proposal(live)
        job.prepare()
        with patch.object(worktree, "_publish", side_effect=publish.Uncertain("lost reply")):
            with self.assertRaises(publish.Uncertain): job.run()  # This real branch operation is already pending.
        controls.last, controls.blocked = job, True
        controls.buttons()

        with patch.object(checkout_pane, "CheckoutChoice", side_effect=AssertionError("second checkout must not start")):
            pane.pick()
        late, late_result = actual_checkout("Late")
        unadopted = late.session
        pane._complete(late, late_result)
        self.assertIs(pane.live, live)
        self.assertIs(controls.last, job)
        self.assertTrue(unadopted.closed)
        self.assertTrue(late_result[0].is_dir())

        # Changing the browser STORE and leaving then selecting the very same archive
        # are normal read operations, not a new working tree or a revived branch source.
        elsewhere = self.root/"browse-only"; elsewhere.mkdir()
        archive.use_store(StoreSelection.open(elsewhere))
        self.assertTrue(live.branch_source_available)
        archive.clear_document()
        self.assertTrue(source.held.closed)
        self.assertIs(pane.live, live)
        self.assertIs(controls.last, job)
        reopened = Selection.open(self.loose, retain=True)
        self.addCleanup(reopened.release_display)
        archive.show_selection(reopened)
        self.assertIsNot(reopened, source)
        self.assertEqual(reopened.observed, source.observed)
        self.assertIn(str(live.store), pane.session_label.text())
        self.assertNotIn(str(elsewhere), pane.session_label.text())
        self.assertFalse(live.branch_source_available)
        self.assertFalse(controls.new_branch.isEnabled())
        self.assertTrue(controls.query.isEnabled())
        self.assertEqual(job.result()[0], "pending")
        attempt = PendingRetry(job); self.addCleanup(attempt.release)
        detail = attempt.prepare()
        self.assertIn(str(live.store), detail)
        self.assertEqual(attempt.operation, job.operation)
        self.assertEqual(attempt.marker, job.proposed)
        attempt.release()  # Only inspecting/approving a replay is not part of closing.

        names_before = set(self.store.iterdir())
        db = live.state/"worktree.sqlite"
        row_before = db.read_bytes()
        with patch.object(worktree, "locked", side_effect=AssertionError("closing must not query or update SQLite")), \
             patch.object(checkout_pane, "confirm", return_value=False) as cancel:
            pane.end.click()
        details = cancel.call_args.args[2]
        for value in (live.work, live.store, live.state, live.checkout_operation, job.operation):
            self.assertIn(str(value), details)
        self.assertIn("不代表已受理或已完成", details)
        self.assertIs(pane.live, live)
        self.assertIs(controls.last, job)

        def active_after_the_question(*_args, **_kw):
            win.pending = object()  # A nested confirmation may no longer own the original idle window.
            return True
        try:
            with patch.object(checkout_pane, "confirm", side_effect=active_after_the_question):
                pane.end.click()
            self.assertIs(pane.live, live)
        finally:
            win.pending = None

        with patch.object(worktree, "locked", side_effect=AssertionError("closing must not query or update SQLite")), \
             patch.object(checkout_pane, "confirm", return_value=True):
            pane.end.click()
        self.assertIsNone(pane.live)
        self.assertTrue(live.closed)
        self.assertIsNone(controls.last)
        self.assertTrue(job.live.closed)
        self.assertTrue(pane.start.isEnabled())
        self.assertFalse(pane.end.isEnabled())
        self.assertIs(pane.snapshot, reopened)
        self.assertEqual(set(self.store.iterdir()), names_before)
        self.assertEqual(db.read_bytes(), row_before)
        self.assertEqual(worktree.status(live.state)["pending_operation"], job.operation)
        self.assertEqual((live.work/"story").read_bytes(), b"original story")
        self.assertTrue((live.state/(job.operation+".zxs")).is_file())
        self.assertFalse(any(elsewhere.iterdir()))

    @unittest.skipUnless(find_spec("PySide6"), "requires optional Qt and a usable display/offscreen platform")
    def test_switching_the_browse_selection_before_or_during_branch_consent_never_enqueues_a_write(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import commit_controls
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        archive, pane = win.archive, win.archive.checkout
        for when in ("before", "inside"):
            archive.show_selection(Selection.open(self.loose))
            live = self.open_live(self.loose, f"Review-{when}")
            pane.live = live
            controls = pane.commits
            controls.set_session(live)
            queued = []
            def queue(work):
                future = Future()
                queued.append((future, work))
                return future
            def approve(*_args, **_kw):
                self.assertEqual(when, "inside")
                archive.show_selection(Selection.open(self.loose))  # Still the same bytes, but a different selection.
                return True
            with self.subTest(when=when), patch.object(win.pool, "submit", side_effect=queue), \
                 patch.object(commit_controls, "confirm", side_effect=approve) as ask:
                controls.new_branch.click()
                future, read_only = queued.pop()
                self.assertTrue(future.set_running_or_notify_cancel())
                future.set_result(read_only())
                if when == "before":
                    archive.show_selection(Selection.open(self.loose))
                win.poll()
                self.assertEqual(ask.call_count, int(when == "inside"))
            self.assertFalse(queued)
            self.assertIs(pane.live, live)
            self.assertIsNone(controls.last)
            self.assertIsNone(controls.running)
            self.assertFalse(controls.new_branch.isEnabled())
            self.assertTrue(controls.save.isEnabled())  # Only the new branch depends on the old browser.
            self.assertIsNone(live.status()["pending_operation"])
            self.assertEqual(list(self.store.iterdir()), [self.original])
            pane.close_session()

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
                self.assertIn("从签出时的原节点", title); self.assertIn(str(self.store), text)
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
