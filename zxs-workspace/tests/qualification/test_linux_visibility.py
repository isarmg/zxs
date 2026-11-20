"""Opt-in Linux source fixtures for moving only the original, still-held clean worktree."""
from concurrent.futures import Future
from contextlib import ExitStack, closing
import errno
from importlib.util import find_spec
import os
from pathlib import Path
import sqlite3
import stat
import sys
import unittest
from unittest.mock import patch

from zipworkspace import lifecycle, worktree
from zipworkspace.desktop.commit_choice import CommitChoice
from zipworkspace.desktop.initialize_choice import InitializeChoice
from zipworkspace.linux_authority import ProcView, owned_directory
from zipworkspace.linux_mount import _ordinary_user
from zipworkspace.model import Rejected, new_id
from zipworkspace.private_freeze import local_mount


@unittest.skipUnless(sys.platform == "linux", "same local Linux ext4/XFS worktree visibility")
class LinuxVisibility(unittest.TestCase):
    def setUp(self):
        spelling = os.environ.get("ZXS_LINUX_VISIBILITY_SCRATCH")
        if not spelling: self.skipTest("supply an exclusive synthetic fixture directory in ZXS_LINUX_VISIBILITY_SCRATCH")
        scratch = Path(spelling)
        if scratch.anchor != "/" or str(scratch) != spelling or ".." in scratch.parts:
            self.fail("give an existing literal absolute path")
        leaf = "visibility-" + new_id()
        with ExitStack() as held:
            proc = ProcView(held)
            uid = _ordinary_user(read_proc=proc.read)
            parent, _ = owned_directory(held, scratch, uid, proc.read)
            local_mount(parent, read_proc=proc.read)
            self.assertEqual(stat.S_IMODE(os.fstat(parent).st_mode), 0o700)
            os.mkdir(leaf, 0o700, dir_fd=parent)
        # Dedicated synthetic cases are retained, not removed by reused pathname.
        root = scratch / leaf
        self.work_parent, self.private, self.store = (root / n for n in ("trees", "states", "archives"))
        for folder in (self.work_parent, self.private, self.store): folder.mkdir(mode=0o700)
        self.tree, self.state = self.work_parent / "work", self.private / "original"
        self.tree.mkdir(mode=0o700)
        (self.tree/"empty").mkdir(mode=0o700)
        (self.tree/"story").write_bytes(b"initial")
        action = InitializeChoice()
        self.addCleanup(action.close)
        action.pick_work(self.tree); action.pick_private(self.private); action.pick_store(self.store)
        action.review(self.state.name); action.run(confirmed=True)
        self.live = action.take_session()
        self.addCleanup(self.live.close)
        self.hidden = self.tree.with_name(".zip-workspace-hidden-" + self.live.instance)

    def commit(self):
        plan = CommitChoice(self.live)
        try:
            plan.prepare()
            self.archive, self.marker = plan.run()
        finally: plan.release()
        self.archive_bytes = self.archive.read_bytes()

    def plan(self, direction, *, prepare=True):
        plan = lifecycle.LiveVisibility(self.live, direction)
        self.addCleanup(plan.close)
        if prepare:
            preview = plan.prepare()
            self.assertIn(str(self.tree), preview)
            self.assertIn(str(self.hidden), preview)
            self.assertIn(str(self.state), preview)
            self.assertIn(str(self.store), preview)
            self.assertIn(self.marker.operation, preview)
            self.assertIn(plan.operation, preview)
        return plan

    def test_round_trip_renames_same_inode_after_observation_and_durable_intent(self):
        self.commit()
        actual = lifecycle.rename_new_at
        seen = []
        def rename(parent, old, new, *, expected_source):
            self.assertTrue(os.path.samestat(os.fstat(parent), self.work_parent.stat()))
            self.assertTrue(os.path.samestat(expected_source, os.fstat(self.live.work_fd)))
            recorded = worktree.status(self.state)
            seen.append((old, new, recorded["pending_visibility_operation"]))
            actual(parent, old, new, expected_source=expected_source)

        hide = self.plan("hide")
        with self.assertRaises(Rejected): hide.run()
        self.assertEqual(self.live.status()["visibility"], "visible")
        self.assertTrue(self.tree.is_dir())
        with patch.object(lifecycle, "rename_new_at", side_effect=rename), \
             patch.object(lifecycle, "observe_held", wraps=lifecycle.observe_held) as scan:
            self.assertEqual(hide.run(confirmed=True), "hidden")
            self.assertTrue(hide.archive.closed)
            self.assertFalse(self.tree.exists())
            self.assertTrue(os.path.samestat(self.hidden.stat(), os.fstat(self.live.work_fd)))
            restore = self.plan("restore")
            self.assertEqual(restore.run(confirmed=True), "visible")
            self.assertEqual(scan.call_count, 2)  # Once per approved action, each with two inventories.
        self.assertEqual(seen, [(self.tree.name, self.hidden.name, hide.operation),
                                (self.hidden.name, self.tree.name, restore.operation)])
        self.assertTrue(os.path.samestat(self.tree.stat(), os.fstat(self.live.work_fd)))
        self.assertEqual(hide.result()[0], "completed")  # The old receipt is not a new hide.
        self.assertEqual(restore.result()[0], "completed")
        self.assertFalse(self.hidden.exists())
        self.assertEqual((self.tree/"story").read_bytes(), b"initial")
        self.assertEqual(list((self.tree/"empty").iterdir()), [])
        self.assertEqual(self.archive.read_bytes(), self.archive_bytes)
        with self.assertRaises(Rejected): hide.run(confirmed=True)

    def test_only_positive_clean_idle_original_without_orphan_pending_sha(self):
        with self.assertRaisesRegex(Rejected, "已提交基线"): self.plan("hide", prepare=False).prepare()
        self.commit()
        dirty = self.plan("hide")
        (self.tree/"story").write_bytes(b"changed")  # Same length, different SHA.
        with patch.object(lifecycle, "_start_visibility", side_effect=AssertionError("dirty never writes intent")):
            with self.assertRaisesRegex(Rejected, "未提交改动"): dirty.run(confirmed=True)
        self.assertFalse(dirty.started)
        self.assertTrue(self.tree.exists())
        (self.tree/"story").write_bytes(b"initial")
        pending = self.plan("hide")
        with worktree.locked(self.state) as (con, _, __):
            con.execute("UPDATE checkout SET pending_sha=? WHERE singleton=1", ("b"*64,))
        with patch.object(lifecycle, "observe_held", side_effect=AssertionError("pending never scans")):
            with self.assertRaisesRegex(Rejected, "无待决工作树"): pending.run(confirmed=True)
        self.assertFalse(pending.started)
        self.assertEqual(self.live.status()["visibility"], "visible")
        self.assertFalse(self.hidden.exists())

    def test_does_not_use_a_replacement_work_parent_even_with_the_same_work_inode(self):
        self.commit()
        plan = self.plan("hide")
        retained = self.work_parent.with_name("original-parent")
        self.work_parent.rename(retained)
        self.work_parent.mkdir(mode=0o700)
        (retained/self.tree.name).rename(self.tree)
        self.assertTrue(os.path.samestat(self.tree.stat(), os.fstat(self.live.work_fd)))
        with patch.object(worktree, "locked", side_effect=AssertionError("wrong parent cannot open the writer")):
            with self.assertRaisesRegex(Rejected, "原父或上级路径已变化"): plan.run(confirmed=True)
        self.assertFalse(plan.started)
        self.assertTrue(self.tree.is_dir())
        self.assertFalse(self.hidden.exists())

    def test_same_archive_bytes_in_a_new_slot_do_not_authorize_this_original_move(self):
        self.commit()
        plan = self.plan("hide")
        saved = self.archive.with_name("preserved.zxs")
        self.archive.rename(saved)
        self.archive.write_bytes(self.archive_bytes)
        with patch.object(worktree, "locked", side_effect=AssertionError("wrong source cannot open the writer")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertFalse(plan.started)
        self.assertEqual(saved.read_bytes(), self.archive.read_bytes())
        self.assertTrue(self.tree.exists())
        self.assertFalse(self.hidden.exists())

    def test_late_empty_original_name_is_never_overwritten_by_restore(self):
        self.commit()
        self.plan("hide").run(confirmed=True)
        restore = self.plan("restore")
        actual, occupied = lifecycle.rename_new_at, []
        def race(*args, **kw):
            self.tree.mkdir(mode=0o700)
            occupied.append(self.tree.stat())
            actual(*args, **kw)
        with patch.object(lifecycle, "rename_new_at", side_effect=race):
            with self.assertRaisesRegex(Rejected, "刚被占用"): restore.run(confirmed=True)
        self.assertTrue(os.path.samestat(self.tree.stat(), occupied[0]))
        self.assertTrue(os.path.samestat(self.hidden.stat(), os.fstat(self.live.work_fd)))
        self.assertEqual(list(self.tree.iterdir()), [])
        outcome, note = restore.result()
        self.assertEqual(outcome, "pending")
        self.assertNotIn("resume-visibility", note)
        self.assertEqual(self.live.status()["pending_visibility_operation"], restore.operation)

    def test_after_move_parent_sync_failure_is_pending_then_original_receipt_may_be_read(self):
        self.commit()
        plan = self.plan("hide")
        real = os.fsync
        def stop(fd):
            if os.path.samestat(os.fstat(fd), self.work_parent.stat()):
                self.assertTrue(os.path.samestat(self.hidden.stat(), os.fstat(self.live.work_fd)))
                raise OSError(errno.EIO, "original parent barrier")
            real(fd)
        with patch.object(lifecycle.os, "fsync", side_effect=stop):
            with self.assertRaisesRegex(OSError, "original parent barrier"): plan.run(confirmed=True)
        with patch.object(lifecycle, "finish", side_effect=AssertionError("query does not resume")), \
             patch.object(lifecycle, "rename_new_at", side_effect=AssertionError("query does not rename")), \
             patch.object(lifecycle.os, "fsync", side_effect=AssertionError("query cannot confirm durability")):
            status, detail = plan.result()
            self.assertEqual(status, "pending")
            self.assertIn("没有完成回执", detail)
            self.assertIn(plan.operation, detail)
        self.assertEqual(self.live.status()["pending_visibility_operation"], plan.operation)
        with patch.object(lifecycle, "rename_new", side_effect=AssertionError("CLI sees exact destination; no second move")):
            lifecycle.finish(self.state, plan.operation)
        self.assertEqual(plan.result()[0], "completed")
        self.assertEqual(self.live.status()["visibility"], "hidden")

    def test_intent_database_directory_barrier_failure_never_moves_or_suggests_resume(self):
        self.commit()
        plan = self.plan("hide")
        actual = os.fsync
        def sync(fd):
            if os.path.samestat(os.fstat(fd), self.state.stat()):
                self.assertEqual(self.live.status()["pending_visibility_operation"], plan.operation)
                self.assertTrue(self.tree.is_dir())
                raise OSError(errno.EIO, "first original STATE barrier")
            actual(fd)
        with patch.object(lifecycle.os, "fsync", side_effect=sync), \
             patch.object(lifecycle, "rename_new_at", side_effect=AssertionError("no move without intent barrier")):
            with self.assertRaisesRegex(OSError, "first original STATE barrier"): plan.run(confirmed=True)
        self.assertTrue(plan.archive.closed)
        self.assertFalse(plan.intent_synced)
        self.assertFalse(self.hidden.exists())
        outcome, readback = plan.result()
        self.assertEqual(outcome, "pending")
        self.assertIn("没有确认原意图已同步", readback)
        self.assertNotIn("resume-visibility", readback)

    def test_failed_state_receipt_barrier_does_not_upgrade_a_visible_completion_row(self):
        self.commit()
        plan = self.plan("hide")
        actual, calls = os.fsync, []
        def sync(fd):
            if os.path.samestat(os.fstat(fd), self.state.stat()):
                calls.append(self.live.status()["visibility"])
                if len(calls) == 2: raise OSError(errno.EIO, "last original STATE barrier")
            actual(fd)
        with patch.object(lifecycle.os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(OSError, "last original STATE barrier"): plan.run(confirmed=True)
        self.assertEqual(calls, ["hide-pending", "hidden"])
        self.assertTrue(plan.archive.closed)
        self.assertTrue(os.path.samestat(self.hidden.stat(), os.fstat(self.live.work_fd)))
        result, note = plan.result()
        self.assertEqual(result, "recorded")
        self.assertIn("没有确认原 STATE", note)
        self.assertNotIn("resume-visibility", note)

    def test_existing_operation_number_cannot_be_taken_as_the_new_decision(self):
        self.commit()
        plan = self.plan("hide")
        with worktree.locked(self.state) as (con, _, __):
            con.execute("INSERT INTO visibility_log VALUES(?,?,?,?)",
                        (plan.operation, "hide", self.marker.tree, "hidden"))
        with patch.object(lifecycle, "observe_held", side_effect=AssertionError("duplicate number never scans")):
            with self.assertRaisesRegex(Rejected, "操作号已在原日志中使用"): plan.run(confirmed=True)
        self.assertTrue(plan.archive.closed)
        self.assertFalse(plan.started)
        self.assertTrue(self.tree.exists())
        with self.assertRaises(Rejected): plan.result()

    def test_foreign_journal_mode_never_moves_or_silently_changes_mode(self):
        self.commit()
        plan = self.plan("hide")
        with closing(sqlite3.connect(self.state/"worktree.sqlite", isolation_level=None)) as native:
            self.assertEqual(native.execute("PRAGMA journal_mode=WAL").fetchone()[0], "wal")
        with patch.object(lifecycle, "observe_held", side_effect=AssertionError("unadmitted journal never scans")):
            with self.assertRaisesRegex(Rejected, "回滚日志模式"): plan.run(confirmed=True)
        with closing(sqlite3.connect(self.state/"worktree.sqlite", isolation_level=None)) as native:
            self.assertEqual(native.execute("PRAGMA journal_mode").fetchone()[0], "wal")
        self.assertFalse(plan.started)
        self.assertTrue(plan.archive.closed)
        self.assertFalse(self.hidden.exists())

    @unittest.skipUnless(find_spec("PySide6"), "requires the optional desktop Qt interface")
    def test_real_window_click_cancel_and_confirm_then_restore_still_same_session(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import checkout_pane
        from zipworkspace.desktop.window import Window
        self.commit()
        app = QApplication.instance() or QApplication([])
        win = Window()
        self.addCleanup(win.close)
        pane = win.archive.checkout
        pane._attach(self.live)
        queue = []
        def submit(job):
            task = Future()
            queue.append((task, job))
            return task
        def drain(n):
            for _ in range(n):
                task, job = queue.pop(0)
                self.assertTrue(task.set_running_or_notify_cancel())
                task.set_result(job())
                win.poll()
            self.assertFalse(queue)
        with patch.object(win.pool, "submit", side_effect=submit), \
             patch.object(checkout_pane, "confirm", side_effect=(False, True, True)) as confirm:
            pane.refresh_live()
            drain(1)
            pane.move.click()
            drain(1)
            self.assertIsNone(pane.visibility_last)
            self.assertTrue(self.tree.is_dir())
            self.assertFalse(self.hidden.exists())
            pane.move.click()
            drain(3)
            self.assertIn("恢复", pane.move.text())
            self.assertFalse(pane.commits.save.isEnabled())
            self.assertTrue(self.hidden.is_dir())
            pane.move_query.click()
            drain(2)
            self.assertIn("历史结果", pane.move_result.text())
            pane.move.click()
            drain(3)
            self.assertTrue(pane.commits.save.isEnabled())
            self.assertTrue(self.tree.is_dir())
            self.assertFalse(self.hidden.exists())
            self.assertIs(pane.live, self.live)
            self.assertEqual(confirm.call_count, 3)
